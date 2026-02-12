use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[derive(Deserialize)]
struct IngestRequest {
    name: String,
    description: Option<String>,
}

#[derive(Serialize)]
struct IngestResponse {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "uploadUrl")]
    upload_url: String,
}

/// POST /memories - Memory ingest
/// Accept memory name/description + S3 pre-signed URL flow
/// Create metadata in DynamoDB memories table (memoryId, userId, name, description, s3Key, sizeBytes, createdAt)
/// Return memoryId + pre-signed upload URL
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = s3::Client::new(&config);
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
    let request_body: IngestRequest = match event.body() {
        Body::Text(text) => serde_json::from_str(text)?,
        Body::Binary(bytes) => serde_json::from_slice(bytes)?,
        Body::Empty => {
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "missing_body",
                    "message": "Request body is required"
                }))?))
                .map_err(Box::new)?);
        }
    };

    // Validate input
    if request_body.name.trim().is_empty() {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "invalid_name",
                "message": "Memory name cannot be empty"
            }))?))
            .map_err(Box::new)?);
    }

    // Generate unique memory ID and S3 key
    let memory_id = Uuid::new_v4().to_string();
    let timestamp = Utc::now();
    let bucket_name = std::env::var("MEMORY_BUCKET")
        .map_err(|_| "MEMORY_BUCKET environment variable not set")?;
    let s3_key = format!("memories/{}/{}.mv", user_id, memory_id);

    // Create metadata record in memories table
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;
    
    let mut item = HashMap::new();
    item.insert("memoryId".to_string(), AttributeValue::S(memory_id.clone()));
    item.insert("userId".to_string(), AttributeValue::S(user_id.to_string()));
    item.insert("name".to_string(), AttributeValue::S(request_body.name.clone()));
    item.insert("s3Key".to_string(), AttributeValue::S(s3_key.clone()));
    item.insert("s3Bucket".to_string(), AttributeValue::S(bucket_name.clone()));
    item.insert("createdAt".to_string(), AttributeValue::S(timestamp.to_rfc3339()));
    item.insert("status".to_string(), AttributeValue::S("pending_upload".to_string()));
    
    if let Some(description) = &request_body.description {
        item.insert("description".to_string(), AttributeValue::S(description.clone()));
    }

    // Initially set sizeBytes to 0 - will be updated after upload
    item.insert("sizeBytes".to_string(), AttributeValue::N("0".to_string()));

    dynamodb_client
        .put_item()
        .table_name(&memories_table)
        .set_item(Some(item))
        .send()
        .await
        .map_err(Box::new)?;

    // Generate pre-signed upload URL (valid for 15 minutes)
    let upload_url = s3_client
        .put_object()
        .bucket(&bucket_name)
        .key(&s3_key)
        .presigned(
            s3::presigning::PresigningConfig::expires_in(
                std::time::Duration::from_secs(15 * 60)
            ).map_err(Box::new)?
        )
        .await
        .map_err(Box::new)?
        .uri()
        .to_string();

    let response = IngestResponse {
        memory_id,
        upload_url,
    };

    let body = serde_json::to_string(&response)?;

    let resp = Response::builder()
        .status(201)
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