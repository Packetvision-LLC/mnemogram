use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
}

#[derive(Serialize)]
struct SearchResult {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "timestamp")]
    timestamp: Option<String>,
    #[serde(rename = "snippet")]
    snippet: String,
    #[serde(rename = "score")]
    score: f64,
}

#[derive(Serialize)]
struct SearchResponse {
    query: String,
    #[serde(rename = "memoryId")]
    memory_id: String,
    results: Vec<SearchResult>,
    total: usize,
}

/// POST /memories/{id}/search - Search within a memory
/// Accept query + memoryId
/// Return search results (placeholder — actual memvid integration comes later)
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    // Extract user ID from authorizer context
    let user_id = event
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            event
                .request_context()
                .authorizer()
                .get("userId")
                .and_then(|v| v.as_str())
        })
        .unwrap_or("anonymous");

    // Extract memory ID from path parameters
    let memory_id = event
        .path_parameters()
        .first("id")
        .or_else(|| event.path_parameters().first("memoryId"))
        .ok_or("Missing memory ID in path")?;

    // Parse request body
    let request_body: SearchRequest = match event.body() {
        Body::Text(text) => serde_json::from_str(text)?,
        Body::Binary(bytes) => serde_json::from_slice(bytes)?,
        Body::Empty => {
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "missing_body",
                    "message": "Request body with 'query' field is required"
                }))?))
                .map_err(Box::new)?);
        }
    };

    // Validate input
    if request_body.query.trim().is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "invalid_query",
                "message": "Search query cannot be empty"
            }))?))
            .map_err(Box::new)?);
    }

    // Verify the memory exists and belongs to the user
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;
    
    let key = HashMap::from([
        ("memoryId".to_string(), AttributeValue::S(memory_id.to_string()))
    ]);

    let get_result = dynamodb_client
        .get_item()
        .table_name(&memories_table)
        .set_key(Some(key))
        .send()
        .await
        .map_err(Box::new)?;

    let memory_item = get_result
        .item
        .ok_or("Memory not found")?;

    // Check if the memory belongs to the user
    let memory_user_id = memory_item
        .get("userId")
        .and_then(|v| v.as_s().ok())
        .ok_or("Invalid memory record")?;

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

    // Check if memory is ready for search
    let status = memory_item
        .get("status")
        .and_then(|v| v.as_s().ok())
        .unwrap_or("unknown");

    if status != "ready" && status != "indexed" {
        return Ok(Response::builder()
            .status(202)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "memory_not_ready",
                "message": format!("Memory is not ready for search. Current status: {}", status),
                "status": status
            }))?))
            .map_err(Box::new)?);
    }

    // TODO: Implement actual search using memvid integration
    // For now, return placeholder results
    let placeholder_results = vec![
        SearchResult {
            memory_id: memory_id.to_string(),
            timestamp: Some("00:01:23".to_string()),
            snippet: format!("This is a placeholder result for query: '{}'", request_body.query),
            score: 0.95,
        },
        SearchResult {
            memory_id: memory_id.to_string(),
            timestamp: Some("00:05:42".to_string()),
            snippet: format!("Another relevant section mentioning: '{}'", request_body.query.to_lowercase()),
            score: 0.87,
        },
    ];

    let response = SearchResponse {
        query: request_body.query,
        memory_id: memory_id.to_string(),
        results: placeholder_results.clone(),
        total: placeholder_results.len(),
    };

    let body = serde_json::to_string(&response)?;

    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(Box::new)?;

    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}