use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::{json, Value};
use shared::memvid::MemvidClient;
use std::collections::HashMap;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

mod extract_user_id;

/// GET /search — semantic search within a specific user memory using S3 Vectors
/// Takes query text and memoryId, searches within that specific memory's vectors
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

    let key = HashMap::from([(
        "memoryId".to_string(),
        AttributeValue::S(memory_id.to_string()),
    )]);

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

    // Check if memory has been migrated to S3 Vectors
    let vectors_migrated = memory_item
        .get("vectorsMigrated")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(false);

    if !vectors_migrated {
        // Memory hasn't been migrated yet - return appropriate response
        return Ok(Response::builder()
            .status(503)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "migration_pending",
                "message": "This memory is being migrated to the new search system. Please try again later.",
                "memoryId": memory_id
            }))?))
            .map_err(Box::new)?);
    }

    // Use S3 Vectors for search
    let default_bucket = std::env::var("MEMORY_BUCKET")
        .or_else(|_| std::env::var("STORAGE_BUCKET"))
        .unwrap_or_default();

    let bucket = memory_item
        .get("s3Bucket")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
        .unwrap_or(&default_bucket);

    let search_results =
        search_memory_with_s3_vectors(s3_client, bucket, memory_id, query_text, limit).await?;

    // Update usage tracking
    let usage_table = std::env::var("USAGE_TABLE").unwrap_or_default();
    let today = Utc::now().format("%Y-%m-%d").to_string();

    if !usage_table.is_empty() {
        let usage_key = HashMap::from([
            ("userId".to_string(), AttributeValue::S(user_id.to_string())),
            ("date".to_string(), AttributeValue::S(today)),
        ]);

        // Increment search counter
        let _usage_result = dynamodb_client
            .update_item()
            .table_name(&usage_table)
            .set_key(Some(usage_key))
            .update_expression("ADD searchCount :inc")
            .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
            .send()
            .await
            .map_err(Box::new)?;
    }

    let body = json!({
        "query": query_text,
        "memoryId": memory_id,
        "userId": user_id,
        "totalResults": search_results.len(),
        "results": search_results,
        "searchedAt": Utc::now().to_rfc3339(),
        "searchMethod": "s3_vectors" // Indicate which search backend was used
    });

    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))
        .map_err(Box::new)?;

    Ok(resp)
}

/// Search within a memory using S3 Vectors
async fn search_memory_with_s3_vectors(
    s3_client: s3::Client,
    bucket: &str,
    memory_id: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Searching memory {} using S3 Vectors with query: '{}'",
        memory_id, query
    );

    // Initialize MemVid client (now backed by S3 Vectors)
    let memvid_client = MemvidClient::new(s3_client, bucket.to_string());

    // Perform search using S3 Vectors
    let search_results = memvid_client
        .search(memory_id, query, limit)
        .await
        .map_err(|e| format!("S3 Vectors search failed: {}", e))?;

    // Convert search results to API format
    let mut api_results = Vec::new();

    for result in search_results {
        api_results.push(json!({
            "memoryId": memory_id,
            "relevanceScore": result.score,
            "snippet": result.snippet,
            "timestamp": result.timestamp,
            "frameId": result.frame_id,
            "confidence": result.score, // Use score as confidence
            "searchMethod": "s3_vectors",
            // Legacy fields for API compatibility
            "s3Key": format!("memories/{}.mv2", memory_id), // For backwards compatibility
            "startTime": 0.0, // Not available in vector search
            "endTime": 0.0     // Not available in vector search
        }));
    }

    info!(
        "S3 Vectors search returned {} results for memory {}",
        api_results.len(),
        memory_id
    );

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
