use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared::memvid::{MemvidClient, MemvidSearchResult};
use shared::errors::MnemogramError;
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
struct RecallRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    20
}

#[derive(Clone, Serialize)]
struct RecallResult {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "memoryName")]
    memory_name: String,
    #[serde(rename = "timestamp")]
    timestamp: Option<String>,
    #[serde(rename = "snippet")]
    snippet: String,
    #[serde(rename = "score")]
    score: f64,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "frameId")]
    frame_id: Option<String>,
}

#[derive(Serialize)]
struct RecallResponse {
    query: String,
    results: Vec<RecallResult>,
    total: usize,
}

impl RecallResult {
    fn from_memvid_result(
        memvid_result: MemvidSearchResult,
        memory_id: &str,
        memory_name: &str,
        created_at: &str,
    ) -> Self {
        RecallResult {
            memory_id: memory_id.to_string(),
            memory_name: memory_name.to_string(),
            timestamp: memvid_result.timestamp,
            snippet: memvid_result.snippet,
            score: memvid_result.score,
            created_at: created_at.to_string(),
            frame_id: memvid_result.frame_id,
        }
    }
}

/// POST /recall - Recall across all memories
/// Accept query
/// Search across user's memories using memvid integration
/// Return aggregated results
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

    // Parse request body
    let request_body: RecallRequest = match event.body() {
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

    // Get all memories for this user from DynamoDB
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;
    
    let bucket = std::env::var("STORAGE_BUCKET")
        .map_err(|_| "STORAGE_BUCKET environment variable not set")?;
    
    // Use a query to find all memories for the user
    let key_condition = "#userId = :userId";
    let filter_condition = "#status = :status1 OR #status = :status2";
    
    let mut expression_attribute_names = HashMap::new();
    expression_attribute_names.insert("#userId".to_string(), "userId".to_string());
    expression_attribute_names.insert("#status".to_string(), "status".to_string());
    
    let mut expression_attribute_values = HashMap::new();
    expression_attribute_values.insert(":userId".to_string(), AttributeValue::S(user_id.to_string()));
    expression_attribute_values.insert(":status1".to_string(), AttributeValue::S("ready".to_string()));
    expression_attribute_values.insert(":status2".to_string(), AttributeValue::S("indexed".to_string()));

    // For now, we'll do a scan since we might not have a GSI on userId yet
    let scan_result = dynamodb_client
        .scan()
        .table_name(&memories_table)
        .filter_expression("userId = :userId AND (#status = :status1 OR #status = :status2)")
        .set_expression_attribute_names(Some(expression_attribute_names))
        .set_expression_attribute_values(Some(expression_attribute_values))
        .send()
        .await
        .map_err(Box::new)?;

    let mut all_results = Vec::new();
    let memvid_client = MemvidClient::new(s3_client, bucket);

    if let Some(items) = scan_result.items {
        for item in items {
            // Extract memory metadata
            let memory_id = item
                .get("memoryId")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            let memory_name = item
                .get("name")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.as_str())
                .unwrap_or("Unnamed Memory");

            let created_at = item
                .get("createdAt")
                .and_then(|v| v.as_s().ok())
                .map(|s| s.as_str())
                .unwrap_or("unknown");

            // Search this memory using memvid
            // Use smaller top_k per memory to stay within total limit
            let per_memory_k = std::cmp::min(8, request_body.top_k);
            
            match memvid_client.search(memory_id, &request_body.query, per_memory_k).await {
                Ok(memvid_results) => {
                    let memory_results: Vec<RecallResult> = memvid_results
                        .into_iter()
                        .map(|result| RecallResult::from_memvid_result(
                            result, memory_id, memory_name, created_at
                        ))
                        .collect();
                    
                    all_results.extend(memory_results);
                }
                Err(e) => {
                    tracing::warn!("Failed to search memory {}: {:?}", memory_id, e);
                    // Continue with other memories rather than failing completely
                }
            }
        }
    }

    // Sort by relevance score (descending)
    all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Limit results to requested top_k
    all_results.truncate(request_body.top_k);

    let response = RecallResponse {
        query: request_body.query,
        results: all_results.clone(),
        total: all_results.len(),
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