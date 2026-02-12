use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

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

    // Get user ID from headers (set by authorizer)
    let user_id = event
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous");

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
        .unwrap_or("");

    let s3_bucket = memory_item
        .get("s3Bucket")
        .and_then(|v| v.as_s().ok())
        .unwrap_or_else(|| std::env::var("MEMORY_BUCKET").unwrap_or_default().as_str());

    // TODO: Download .mv2 file from S3 and use memvid-core to search
    // For now, return mock search results
    let search_results = search_memory_placeholder(
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

/// Placeholder function for searching a memory file using memvid-core
/// TODO: Replace with actual memvid-core integration
async fn search_memory_placeholder(
    _s3_client: &s3::Client,
    _s3_bucket: &str,
    s3_key: &str,
    query: &str,
    memory_id: &str,
    limit: usize,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    // Simulate semantic search with mock results
    // In reality, this would:
    // 1. Download the .mv2 file from S3
    // 2. Load it with memvid-core
    // 3. Perform BM25+vector hybrid search
    // 4. Return ranked results with relevance scores and timestamps
    
    let mut mock_results = vec![];
    
    for i in 0..std::cmp::min(limit, 10) {
        mock_results.push(json!({
            "memoryId": memory_id,
            "relevanceScore": 0.95 - (i as f64 * 0.05),
            "snippet": format!("Mock search result {} for query '{}' in memory {}", i + 1, query, memory_id),
            "timestamp": format!("2024-02-12T{}:{}:00Z", 17 - (i / 4), 30 + (i % 4) * 10),
            "s3Key": s3_key,
            "chunkId": format!("chunk_{}", i),
            "startTime": i as f64 * 10.5,
            "endTime": (i as f64 * 10.5) + 8.3,
            "confidence": 0.88 - (i as f64 * 0.03)
        }));
    }

    Ok(mock_results)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}