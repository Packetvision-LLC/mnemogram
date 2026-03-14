use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::Serialize;
use serde_json::json;
use shared::errors::MnemogramError;
use shared::memvid::{MemvidClient, MemvidSearchResult};
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Serialize)]
struct RecallResult {
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
    #[serde(rename = "metadata", skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct BulkRecallResult {
    #[serde(rename = "memoryId")]
    memory_id: String,
    name: String,
    #[serde(rename = "totalChunks")]
    total_chunks: usize,
    #[serde(rename = "retrievedChunks")]
    retrieved_chunks: usize,
    chunks: Vec<RecallResult>,
}

#[derive(Serialize)]
struct RecallResponse {
    #[serde(rename = "memoryId")]
    memory_id: String,
    results: Vec<RecallResult>,
    total: usize,
    offset: usize,
    limit: usize,
    #[serde(rename = "hasMore")]
    has_more: bool,
    #[serde(rename = "retrievedAt")]
    retrieved_at: String,
}

#[derive(Serialize)]
struct BulkRecallResponse {
    #[serde(rename = "userId")]
    user_id: String,
    memories: Vec<BulkRecallResult>,
    #[serde(rename = "totalMemories")]
    total_memories: usize,
    #[serde(rename = "totalChunks")]
    total_chunks: usize,
    #[serde(rename = "retrievedAt")]
    retrieved_at: String,
}

/// Memory retrieval operations using S3 Vectors
/// Supports both individual memory recall and bulk retrieval
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    let s3_client = S3Client::new(&config);

    // Extract user ID from authorizer context
    let user_id = event
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("anonymous");

    // Determine operation type from path
    let path = event.uri().path().to_string();

    match path {
        path if path.contains("/memories/") && path.ends_with("/recall") => {
            // Single memory recall: GET /memories/{id}/recall
            handle_memory_recall(&event, s3_client, dynamodb_client, user_id).await
        }
        path if path.ends_with("/recall") => {
            // Bulk recall: GET /recall or POST /recall
            handle_bulk_recall(&event, s3_client, dynamodb_client, user_id).await
        }
        _ => Ok(Response::builder()
            .status(404)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "not_found",
                "message": "Endpoint not found"
            }))?))
            .map_err(Box::new)?),
    }
}

/// Handle single memory recall by ID
async fn handle_memory_recall(
    event: &Request,
    s3_client: S3Client,
    dynamodb_client: aws_sdk_dynamodb::Client,
    user_id: &str,
) -> Result<Response<Body>, Error> {
    // Extract memory ID from path parameters
    let path_params = event.path_parameters();
    let memory_id = path_params
        .first("id")
        .or_else(|| path_params.first("memoryId"))
        .ok_or("Missing memory ID in path")?;

    // Parse query parameters
    let query_params = event.query_string_parameters();
    let limit: usize = query_params
        .first("limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50)
        .min(1000); // Cap at 1000

    let offset: usize = query_params
        .first("offset")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let include_metadata = query_params
        .first("include_metadata")
        .map(|s| s == "true")
        .unwrap_or(false);

    // Verify memory exists and belongs to user
    let memory_info = verify_memory_access(&dynamodb_client, memory_id, user_id).await?;

    // Check if memory has been migrated to S3 Vectors
    if !memory_info.vectors_migrated {
        return Ok(Response::builder()
            .status(503)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "migration_pending",
                "message": "This memory is being migrated to S3 Vectors. Please try again later.",
                "memoryId": memory_id
            }))?))
            .map_err(Box::new)?);
    }

    // Initialize MemVid client with S3 Vectors backend
    let bucket = std::env::var("STORAGE_BUCKET")
        .or_else(|_| std::env::var("MEMORY_BUCKET"))
        .map_err(|_| "STORAGE_BUCKET environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client, bucket);

    // Retrieve memory content
    let all_results = match memvid_client
        .retrieve_memory(memory_id, Some(limit + offset + 100))
        .await
    {
        Ok(results) => results,
        Err(MnemogramError::ExternalService(msg)) => {
            tracing::error!("Memory retrieval failed: {}", msg);
            return Ok(Response::builder()
                .status(503)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "retrieval_unavailable",
                    "message": "Memory retrieval temporarily unavailable"
                }))?))
                .map_err(Box::new)?);
        }
        Err(e) => {
            tracing::error!("Unexpected error during retrieval: {:?}", e);
            return Err(format!("Retrieval failed: {:?}", e).into());
        }
    };

    // Apply offset and limit
    let paginated_results: Vec<MemvidSearchResult> =
        all_results.into_iter().skip(offset).take(limit).collect();

    // Convert to API format
    let results: Vec<RecallResult> = paginated_results
        .iter()
        .map(|result| RecallResult {
            memory_id: memory_id.to_string(),
            timestamp: result.timestamp.clone(),
            snippet: result.snippet.clone(),
            score: result.score,
            frame_id: result.frame_id.clone(),
            metadata: if include_metadata {
                Some(json!({
                    "source": "s3_vectors",
                    "retrievalMethod": "vector_similarity"
                }))
            } else {
                None
            },
        })
        .collect();

    let has_more = results.len() == limit;

    let response = RecallResponse {
        memory_id: memory_id.to_string(),
        results,
        total: paginated_results.len(),
        offset,
        limit,
        has_more,
        retrieved_at: chrono::Utc::now().to_rfc3339(),
    };

    let body = serde_json::to_string(&response)?;

    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(Box::new)?)
}

/// Handle bulk memory retrieval for a user
async fn handle_bulk_recall(
    event: &Request,
    s3_client: S3Client,
    dynamodb_client: aws_sdk_dynamodb::Client,
    user_id: &str,
) -> Result<Response<Body>, Error> {
    // Parse query parameters
    let query_params = event.query_string_parameters();
    let max_memories: usize = query_params
        .first("max_memories")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10)
        .min(50); // Cap at 50 memories for bulk operations

    let chunks_per_memory: usize = query_params
        .first("chunks_per_memory")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20)
        .min(100); // Cap chunks per memory

    // Get user's migrated memories
    let user_memories = get_user_memories(&dynamodb_client, user_id, max_memories).await?;

    if user_memories.is_empty() {
        return Ok(Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&BulkRecallResponse {
                user_id: user_id.to_string(),
                memories: vec![],
                total_memories: 0,
                total_chunks: 0,
                retrieved_at: chrono::Utc::now().to_rfc3339(),
            })?))
            .map_err(Box::new)?);
    }

    // Initialize MemVid client
    let bucket = std::env::var("STORAGE_BUCKET")
        .or_else(|_| std::env::var("MEMORY_BUCKET"))
        .map_err(|_| "STORAGE_BUCKET environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client, bucket);

    // Retrieve chunks for each memory
    let mut bulk_results = Vec::new();
    let mut total_chunks = 0;

    for memory in &user_memories {
        if !memory.vectors_migrated {
            // Skip non-migrated memories
            bulk_results.push(BulkRecallResult {
                memory_id: memory.memory_id.clone(),
                name: memory.name.clone(),
                total_chunks: 0,
                retrieved_chunks: 0,
                status: "migration_pending".to_string(),
                chunks: vec![],
            });
            continue;
        }

        match memvid_client
            .retrieve_memory(&memory.memory_id, Some(chunks_per_memory))
            .await
        {
            Ok(results) => {
                let chunks: Vec<RecallResult> = results
                    .iter()
                    .map(|result| RecallResult {
                        memory_id: memory.memory_id.clone(),
                        timestamp: result.timestamp.clone(),
                        snippet: result.snippet.clone(),
                        score: result.score,
                        frame_id: result.frame_id.clone(),
                        metadata: None, // Don't include metadata in bulk operations
                    })
                    .collect();

                total_chunks += chunks.len();

                bulk_results.push(BulkRecallResult {
                    memory_id: memory.memory_id.clone(),
                    name: memory.name.clone(),
                    total_chunks: results.len(),
                    retrieved_chunks: chunks.len(),
                    status: "success".to_string(),
                    chunks,
                });
            }
            Err(e) => {
                tracing::warn!("Failed to retrieve memory {}: {}", memory.memory_id, e);
                bulk_results.push(BulkRecallResult {
                    memory_id: memory.memory_id.clone(),
                    name: memory.name.clone(),
                    total_chunks: 0,
                    retrieved_chunks: 0,
                    status: "error".to_string(),
                    chunks: vec![],
                });
            }
        }
    }

    let response = BulkRecallResponse {
        user_id: user_id.to_string(),
        memories: bulk_results,
        total_memories: user_memories.len(),
        total_chunks,
        retrieved_at: chrono::Utc::now().to_rfc3339(),
    };

    let body = serde_json::to_string(&response)?;

    Ok(Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .map_err(Box::new)?)
}

#[derive(Debug)]
struct MemoryInfo {
    memory_id: String,
    name: String,
    vectors_migrated: bool,
}

/// Verify user has access to memory and get basic info
async fn verify_memory_access(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    memory_id: &str,
    user_id: &str,
) -> Result<MemoryInfo, Box<dyn std::error::Error + Send + Sync>> {
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
        return Err("Access denied - memory does not belong to user".into());
    }

    let name = memory_item
        .get("name")
        .and_then(|v| v.as_s().ok().map(ToString::to_string))
        .unwrap_or_else(|| "Untitled Memory".to_string());

    let vectors_migrated = memory_item
        .get("vectorsMigrated")
        .and_then(|v| v.as_bool().ok())
        .copied()
        .unwrap_or(false);

    Ok(MemoryInfo {
        memory_id: memory_id.to_string(),
        name,
        vectors_migrated,
    })
}

/// Get list of user's memories (only migrated ones for retrieval)
async fn get_user_memories(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    user_id: &str,
    max_memories: usize,
) -> Result<Vec<MemoryInfo>, Box<dyn std::error::Error + Send + Sync>> {
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

    let mut memories = Vec::new();
    let mut last_evaluated_key = None;

    loop {
        let mut query = dynamodb_client
            .query()
            .table_name(&memories_table)
            .index_name("userId-createdAt-index") // Assume GSI exists
            .key_condition_expression("userId = :userId")
            .expression_attribute_values(":userId", AttributeValue::S(user_id.to_string()))
            .limit(50); // Query in batches

        if let Some(key) = last_evaluated_key {
            query = query.set_exclusive_start_key(Some(key));
        }

        let result = query.send().await.map_err(Box::new)?;

        if let Some(items) = result.items {
            for item in items {
                let memory_id = item
                    .get("memoryId")
                    .and_then(|v| v.as_s().ok().map(ToString::to_string))
                    .unwrap_or_else(|| "".to_string());

                let name = item
                    .get("name")
                    .and_then(|v| v.as_s().ok().map(ToString::to_string))
                    .unwrap_or_else(|| "Untitled Memory".to_string());

                let vectors_migrated = item
                    .get("vectorsMigrated")
                    .and_then(|v| v.as_bool().ok())
                    .copied()
                    .unwrap_or(false);

                memories.push(MemoryInfo {
                    memory_id,
                    name,
                    vectors_migrated,
                });

                if memories.len() >= max_memories {
                    break;
                }
            }
        }

        last_evaluated_key = result.last_evaluated_key;

        if last_evaluated_key.is_none() || memories.len() >= max_memories {
            break;
        }
    }

    Ok(memories)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}
