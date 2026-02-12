use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
struct BatchRecallRequest {
    queries: Vec<String>,
    #[serde(rename = "memoryId")]
    memory_id: Option<String>,
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
}

#[derive(Serialize)]
struct QueryResult {
    query: String,
    matches: Vec<RecallResult>,
}

#[derive(Serialize)]
struct BatchRecallResponse {
    results: Vec<QueryResult>,
}

/// POST /v1/batch-recall - Batch recall across memories
/// Accept multiple queries and return results for each
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

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
    let request_body: BatchRecallRequest = match event.body() {
        Body::Text(text) => serde_json::from_str(text)?,
        Body::Binary(bytes) => serde_json::from_slice(bytes)?,
        Body::Empty => {
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "missing_body",
                    "message": "Request body with 'queries' field is required"
                }))?))
                .map_err(Box::new)?);
        }
    };

    // Validate input
    if request_body.queries.is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "empty_queries",
                "message": "At least one query is required"
            }))?))
            .map_err(Box::new)?);
    }

    if request_body.queries.len() > 10 {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "too_many_queries",
                "message": "Maximum 10 queries allowed per batch request"
            }))?))
            .map_err(Box::new)?);
    }

    // Validate that all queries are non-empty
    for query in &request_body.queries {
        if query.trim().is_empty() {
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "invalid_query",
                    "message": "All search queries must be non-empty"
                }))?))
                .map_err(Box::new)?);
        }
    }

    // Get memories for this user from DynamoDB (single query)
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;
    
    let mut expression_attribute_names = HashMap::new();
    expression_attribute_names.insert("#userId".to_string(), "userId".to_string());
    expression_attribute_names.insert("#status".to_string(), "status".to_string());
    
    let mut expression_attribute_values = HashMap::new();
    expression_attribute_values.insert(":userId".to_string(), AttributeValue::S(user_id.to_string()));
    expression_attribute_values.insert(":status1".to_string(), AttributeValue::S("ready".to_string()));
    expression_attribute_values.insert(":status2".to_string(), AttributeValue::S("indexed".to_string()));

    // If a specific memoryId is provided, filter to that memory only
    let mut filter_expression = "userId = :userId AND (#status = :status1 OR #status = :status2)".to_string();
    if let Some(memory_id) = &request_body.memory_id {
        filter_expression.push_str(" AND memoryId = :memoryId");
        expression_attribute_values.insert(":memoryId".to_string(), AttributeValue::S(memory_id.clone()));
    }

    let scan_result = dynamodb_client
        .scan()
        .table_name(&memories_table)
        .filter_expression(&filter_expression)
        .set_expression_attribute_names(Some(expression_attribute_names))
        .set_expression_attribute_values(Some(expression_attribute_values))
        .send()
        .await
        .map_err(Box::new)?;

    let mut batch_results = Vec::new();

    // Process each query
    for query in &request_body.queries {
        let mut query_results = Vec::new();

        if let Some(ref items) = scan_result.items {
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

                // TODO: Implement actual search using memvid integration
                // For now, return placeholder results for each memory
                let memory_results = search_memory_placeholder(
                    query,
                    memory_id,
                    memory_name,
                    created_at
                );
                
                query_results.extend(memory_results);
            }
        }

        // Sort by relevance score (descending) and limit to top 20 per query
        query_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        query_results.truncate(20);

        batch_results.push(QueryResult {
            query: query.clone(),
            matches: query_results,
        });
    }

    let response = BatchRecallResponse {
        results: batch_results,
    };

    let body = serde_json::to_string(&response)?;

    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(Box::new)?;

    Ok(resp)
}

/// Placeholder function for searching a single memory file
/// TODO: Replace with actual memvid-core integration
fn search_memory_placeholder(
    query: &str,
    memory_id: &str,
    memory_name: &str,
    created_at: &str,
) -> Vec<RecallResult> {
    // Simulate searching within this memory
    // In reality, this would:
    // 1. Load the memory from S3 using memvid-core (single S3 GET for all queries)
    // 2. Perform semantic search for each query
    // 3. Return ranked results with timestamps
    
    vec![
        RecallResult {
            memory_id: memory_id.to_string(),
            memory_name: memory_name.to_string(),
            timestamp: Some("00:02:15".to_string()),
            snippet: format!("Relevant content about '{}' found in {}", query, memory_name),
            score: 0.92,
            created_at: created_at.to_string(),
        },
        RecallResult {
            memory_id: memory_id.to_string(),
            memory_name: memory_name.to_string(),
            timestamp: Some("00:08:42".to_string()),
            snippet: format!("Another section mentioning '{}'", query.to_lowercase()),
            score: 0.78,
            created_at: created_at.to_string(),
        },
    ]
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}