use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::errors::MnemogramError;
use shared::memvid::{MemvidClient, MemvidSearchResult};
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    10
}

#[derive(Clone, Serialize)]
struct SearchResult {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "timestamp")]
    timestamp: Option<String>,
    #[serde(rename = "snippet")]
    snippet: String,
    #[serde(rename = "score")]
    score: f64,
    #[serde(rename = "frameId")]
    frame_id: Option<String>,
}

#[derive(Serialize)]
struct SearchResponse {
    query: String,
    #[serde(rename = "memoryId")]
    memory_id: String,
    results: Vec<SearchResult>,
    total: usize,
    #[serde(rename = "searchMethod")]
    search_method: String,
}

impl From<MemvidSearchResult> for SearchResult {
    fn from(memvid_result: MemvidSearchResult) -> Self {
        SearchResult {
            memory_id: String::new(), // Will be set by caller
            timestamp: memvid_result.timestamp,
            snippet: memvid_result.snippet,
            score: memvid_result.score,
            frame_id: memvid_result.frame_id,
        }
    }
}

/// POST /memories/{id}/search - Search within a memory using S3 Vectors
/// Accept query + memoryId
/// Return search results using S3 Vectors integration
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    let s3_client = S3Client::new(&config);

    // Extract user ID from authorizer context or headers
    let user_id = event
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            // Try to get from request context if available
            if let Some(_context) = event.request_context().authorizer() {
                // Note: We'll need to implement proper authorizer context parsing
                // For now, just use a placeholder
                None
            } else {
                None
            }
        })
        .unwrap_or("anonymous");

    // Extract memory ID from path parameters
    let path_params = event.path_parameters();
    let memory_id = path_params
        .first("id")
        .or_else(|| path_params.first("memoryId"))
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

    let memory_item = get_result.item.ok_or("Memory not found")?;

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

    // Check if memory has been migrated to S3 Vectors
    let vectors_migrated = memory_item
        .get("vectorsMigrated")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(false);

    // Check if memory is ready for search
    let status = memory_item
        .get("status")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
        .unwrap_or("unknown");

    if !vectors_migrated {
        return Ok(Response::builder()
            .status(503)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "migration_pending",
                "message": "This memory is being migrated to S3 Vectors. Please try again later.",
                "status": status
            }))?))
            .map_err(Box::new)?);
    }

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

    // Initialize S3 Vectors client
    let bucket = std::env::var("STORAGE_BUCKET")
        .or_else(|_| std::env::var("MEMORY_BUCKET"))
        .map_err(|_| "STORAGE_BUCKET or MEMORY_BUCKET environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client, bucket);

    // Perform search using S3 Vectors
    let memvid_results = match memvid_client
        .search(memory_id, &request_body.query, request_body.top_k)
        .await
    {
        Ok(results) => results,
        Err(MnemogramError::S3Error(msg)) | Err(MnemogramError::ExternalService(msg)) => {
            tracing::error!("S3 Vectors search failed: {}", msg);

            return Ok(Response::builder()
                .status(503)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "search_unavailable",
                    "message": "Search service temporarily unavailable"
                }))?))
                .map_err(Box::new)?);
        }
        Err(e) => {
            tracing::error!("Unexpected error during search: {:?}", e);
            return Err(format!("Search failed: {:?}", e).into());
        }
    };

    // Convert S3 Vectors results to API format
    let results: Vec<SearchResult> = memvid_results
        .into_iter()
        .map(|result| {
            let mut search_result = SearchResult::from(result);
            search_result.memory_id = memory_id.to_string();
            search_result
        })
        .collect();

    let response = SearchResponse {
        query: request_body.query,
        memory_id: memory_id.to_string(),
        results: results.clone(),
        total: results.len(),
        search_method: "s3_vectors".to_string(),
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
