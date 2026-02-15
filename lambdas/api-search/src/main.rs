use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{info, warn, error};
use tracing_subscriber::EnvFilter;

mod extract_user_id;

/// GET /search — semantic search within a specific user memory
/// Takes query text and memoryId, searches within that specific .mv2 file
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = s3::Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    // Extract query parameters
    let query_params = event.query_string_parameters();
    let query_text = query_params.first("q").unwrap_or("");
    let memory_id = query_params.first("memoryId").unwrap_or("");
    let limit: usize = query_params
        .first("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .min(100); // Cap at 100 results

    if query_text.is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "missing_query",
                "message": "Query parameter 'q' is required"
            }))?))
            .map_err(Box::new)?);
    }

    if memory_id.is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "missing_memory_id",
                "message": "Query parameter 'memoryId' is required"
            }))?))
            .map_err(Box::new)?);
    }

    // Get user ID from request context (set by authorizer)
    let user_id = match extract_user_id::extract_user_id_from_context(&event) {
        Ok(id) => id,
        Err(err) => {
            error!("Failed to extract user ID: {}", err);
            return Ok(Response::builder()
                .status(401)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "unauthorized",
                    "message": "Valid authorization required"
                }))?))
                .map_err(Box::new)?);
        }
    };

    // Get memory metadata from DynamoDB
    let memories_table = std::env::var("MEMORIES_TABLE").unwrap_or_default();
    
    let key = HashMap::from([
        ("memoryId".to_string(), AttributeValue::S(memory_id.to_string())),
    ]);

    let get_result = dynamodb_client
        .get_item()
        .table_name(&memories_table)
        .set_key(Some(key))
        .send()
        .await
        .map_err(Box::new)?;

    let memory_item = match get_result.item {
        Some(item) => item,
        None => {
            return Ok(Response::builder()
                .status(404)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "memory_not_found",
                    "message": "Memory with specified ID not found"
                }))?))
                .map_err(Box::new)?);
        }
    };

    // Verify user owns this memory
    let memory_user_id = memory_item
        .get("userId")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
        .unwrap_or("");
    
    if memory_user_id != user_id {
        return Ok(Response::builder()
            .status(403)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "access_denied",
                "message": "You don't have access to this memory"
            }))?))
            .map_err(Box::new)?);
    }

    // Get S3 key for the .mv2 file
    let s3_key = memory_item
        .get("s3Key")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
        .unwrap_or("");

    let default_bucket = std::env::var("MEMORY_BUCKET").unwrap_or_default();
    let s3_bucket = memory_item
        .get("s3Bucket")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
        .unwrap_or(&default_bucket);

    // Download .mv2 file from S3 and use memvid-core to search
    let search_results = search_memory_with_memvid(
        &s3_client,
        s3_bucket,
        s3_key,
        query_text,
        memory_id,
        limit,
    ).await?;

    // Update usage tracking
    let usage_table = std::env::var("USAGE_TABLE").unwrap_or_default();
    let today = Utc::now().format("%Y-%m-%d").to_string();
    
    let usage_key = HashMap::from([
        ("userId".to_string(), AttributeValue::S(user_id.to_string())),
        ("date".to_string(), AttributeValue::S(today)),
    ]);

    // Increment search counter (simplified)
    let _usage_result = dynamodb_client
        .update_item()
        .table_name(&usage_table)
        .set_key(Some(usage_key))
        .update_expression("ADD searchCount :inc")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .send()
        .await
        .map_err(Box::new)?;

    let body = json!({
        "query": query_text,
        "memoryId": memory_id,
        "userId": user_id,
        "totalResults": search_results.len(),
        "results": search_results,
        "searchedAt": Utc::now().to_rfc3339()
    });

    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))
        .map_err(Box::new)?;

    Ok(resp)
}

/// Search within a memory file using memvid-core
async fn search_memory_with_memvid(
    s3_client: &s3::Client,
    s3_bucket: &str,
    s3_key: &str,
    query: &str,
    memory_id: &str,
    limit: usize,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    info!("Downloading memory file from S3: s3://{}/{}", s3_bucket, s3_key);
    
    // Create a temporary file to store the downloaded .mv2 file
    let temp_file = NamedTempFile::new()?;
    let temp_path = temp_file.path();
    
    // Download the .mv2 file from S3
    let get_object_result = s3_client
        .get_object()
        .bucket(s3_bucket)
        .key(s3_key)
        .send()
        .await
        .map_err(|e| format!("Failed to download .mv2 file from S3: {}", e))?;
    
    let mut body = get_object_result.body.into_async_read();
    let mut temp_file_write = File::create(temp_path).await?;
    
    // Stream the S3 object to the temporary file
    tokio::io::copy(&mut body, &mut temp_file_write).await?;
    temp_file_write.flush().await?;
    
    info!("Downloaded memory file to: {:?}", temp_path);
    
    // Find the memvid binary
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };
    
    info!("Using memvid binary: {}", memvid_path);
    
    // Run memvid find command
    let output = Command::new(memvid_path)
        .arg("find")
        .arg("--query")
        .arg(query)
        .arg("--json")
        .arg("--top-k")
        .arg(limit.to_string())
        .arg("--mode")
        .arg("auto") // Use both lexical and semantic search
        .arg(temp_path)
        .output()
        .await
        .map_err(|e| format!("Failed to execute memvid command: {}", e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("Memvid search failed: {}", stderr);
        return Err(format!("Memvid search failed: {}", stderr).into());
    }
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    info!("Memvid output: {}", stdout);
    
    // Parse the JSON output from memvid
    let memvid_results: Value = serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse memvid JSON output: {}", e))?;
    
    let mut api_results = Vec::new();
    
    // Convert memvid results to API format
    if let Some(results_array) = memvid_results.get("results").and_then(|r| r.as_array()) {
        for result in results_array {
            let snippet = result.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
            let score = result.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
            let frame_id = result.get("frame_id").and_then(|f| f.as_str()).unwrap_or("");
            let timestamp = result.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
            let start_time = result.get("start_time").and_then(|s| s.as_f64()).unwrap_or(0.0);
            let end_time = result.get("end_time").and_then(|e| e.as_f64()).unwrap_or(0.0);
            
            api_results.push(json!({
                "memoryId": memory_id,
                "relevanceScore": score,
                "snippet": snippet,
                "timestamp": timestamp,
                "s3Key": s3_key,
                "frameId": frame_id,
                "startTime": start_time,
                "endTime": end_time,
                "confidence": score // Use relevance score as confidence
            }));
        }
    } else if let Some(results_array) = memvid_results.as_array() {
        // Handle case where root is an array
        for result in results_array {
            let snippet = result.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
            let score = result.get("score").and_then(|s| s.as_f64()).unwrap_or(0.0);
            let frame_id = result.get("frame_id").and_then(|f| f.as_str()).unwrap_or("");
            let timestamp = result.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
            let start_time = result.get("start_time").and_then(|s| s.as_f64()).unwrap_or(0.0);
            let end_time = result.get("end_time").and_then(|e| e.as_f64()).unwrap_or(0.0);
            
            api_results.push(json!({
                "memoryId": memory_id,
                "relevanceScore": score,
                "snippet": snippet,
                "timestamp": timestamp,
                "s3Key": s3_key,
                "frameId": frame_id,
                "startTime": start_time,
                "endTime": end_time,
                "confidence": score // Use relevance score as confidence
            }));
        }
    } else {
        warn!("Unexpected memvid output format: {}", memvid_results);
    }
    
    info!("Converted {} memvid results to API format", api_results.len());
    Ok(api_results)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}