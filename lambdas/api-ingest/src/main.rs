use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::json;
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

/// PUT /memories — ingest content into a .mv2 memory file
/// Accepts .mv2 file upload via multipart or pre-signed URL flow
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = s3::Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    // Extract user ID from headers or query params (set by authorizer)
    let user_id = event
        .headers()
        .get("x-user-id")
        .and_then(|v| v.to_str().ok())
        .or_else(|| event.query_string_parameters().first("userId"))
        .unwrap_or("anonymous");

    // Generate a unique memory ID
    let memory_id = Uuid::new_v4().to_string();
    let timestamp = Utc::now();
    
    // Get file metadata from headers
    let content_type = event
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream");
    
    let content_length = event
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    // Validate file size (limit to 100MB for now)
    if content_length > 100 * 1024 * 1024 {
        return Ok(Response::builder()
            .status(413)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "file_too_large",
                "message": "File size exceeds 100MB limit",
                "maxSize": "100MB"
            }))?))
            .map_err(Box::new)?);
    }

    // Validate file type (should be .mv2)
    if !content_type.contains("mv2") && !event.uri().path().ends_with(".mv2") {
        return Ok(Response::builder()
            .status(400)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&json!({
                "error": "invalid_file_type",
                "message": "Only .mv2 memory files are supported",
                "receivedType": content_type
            }))?))
            .map_err(Box::new)?);
    }

    let bucket_name = std::env::var("MEMORY_BUCKET").unwrap_or_default();
    let s3_key = format!("{}/{}.mv2", user_id, memory_id);

    // Store file in S3 (for now, this is a placeholder - in reality we'd handle multipart upload)
    let body_bytes = match event.body() {
        Body::Binary(bytes) => bytes.clone(),
        Body::Text(text) => text.as_bytes().to_vec(),
        Body::Empty => Vec::new(),
    };

    // TODO: In a real implementation, we'd handle multipart uploads properly
    // This is a simplified version for demonstration
    if !body_bytes.is_empty() {
        let _put_result = s3_client
            .put_object()
            .bucket(&bucket_name)
            .key(&s3_key)
            .body(aws_sdk_s3::primitives::ByteStream::from(body_bytes))
            .content_type(content_type)
            .send()
            .await
            .map_err(Box::new)?;
    }

    // Create metadata record in memories table
    let memories_table = std::env::var("MEMORIES_TABLE").unwrap_or_default();
    
    let mut item = HashMap::new();
    item.insert("memoryId".to_string(), AttributeValue::S(memory_id.clone()));
    item.insert("userId".to_string(), AttributeValue::S(user_id.to_string()));
    item.insert("s3Key".to_string(), AttributeValue::S(s3_key.clone()));
    item.insert("s3Bucket".to_string(), AttributeValue::S(bucket_name.clone()));
    item.insert("contentType".to_string(), AttributeValue::S(content_type.to_string()));
    item.insert("fileSize".to_string(), AttributeValue::N(content_length.to_string()));
    item.insert("createdAt".to_string(), AttributeValue::S(timestamp.to_rfc3339()));
    item.insert("status".to_string(), AttributeValue::S("processing".to_string()));

    let _put_item_result = dynamodb_client
        .put_item()
        .table_name(&memories_table)
        .set_item(Some(item))
        .send()
        .await
        .map_err(Box::new)?;

    // TODO: Trigger background processing of .mv2 file (index creation, etc.)
    
    // Update usage tracking (simplified)
    let usage_table = std::env::var("USAGE_TABLE").unwrap_or_default();
    let today = timestamp.format("%Y-%m-%d").to_string();
    
    let usage_key = HashMap::from([
        ("userId".to_string(), AttributeValue::S(user_id.to_string())),
        ("date".to_string(), AttributeValue::S(today)),
    ]);

    // Increment usage counter (simplified - should use atomic updates)
    let _usage_result = dynamodb_client
        .update_item()
        .table_name(&usage_table)
        .set_key(Some(usage_key))
        .update_expression("ADD uploadCount :inc, bytesUploaded :bytes")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":bytes", AttributeValue::N(content_length.to_string()))
        .send()
        .await
        .map_err(Box::new)?;

    let body = json!({
        "memoryId": memory_id,
        "userId": user_id,
        "s3Key": s3_key,
        "status": "processing",
        "fileSize": content_length,
        "createdAt": timestamp.to_rfc3339(),
        "message": "Memory file uploaded successfully and is being processed"
    });

    let resp = Response::builder()
        .status(201)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))
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