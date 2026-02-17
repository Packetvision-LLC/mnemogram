use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use aws_sdk_sqs;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::json;
use std::collections::HashMap;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod extract_user_id;

/// PUT /memories — ingest content into a .mv2 memory file
/// Accepts .mv2 file upload via multipart or pre-signed URL flow
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = s3::Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    let sqs_client = aws_sdk_sqs::Client::new(&config);

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

    // Handle different upload modes based on request
    let upload_mode = event
        .headers()
        .get("x-upload-mode")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("direct");

    let (file_size, upload_url) = match upload_mode {
        "presigned" => {
            // Generate a pre-signed URL for large file uploads
            info!("Generating pre-signed URL for memory: {}", memory_id);

            let presigned_req = s3_client
                .put_object()
                .bucket(&bucket_name)
                .key(&s3_key)
                .content_type(content_type);

            let presigned_url = presigned_req
                .presigned(aws_sdk_s3::presigning::PresigningConfig::expires_in(
                    std::time::Duration::from_secs(3600), // 1 hour expiry
                )?)
                .await
                .map_err(|e| format!("Failed to generate pre-signed URL: {}", e))?;

            (0, Some(presigned_url.uri().to_string()))
        }
        _ => {
            // Direct upload through Lambda (for smaller files)
            info!("Processing direct upload for memory: {}", memory_id);

            let body_bytes = match event.body() {
                Body::Binary(bytes) => bytes.clone(),
                Body::Text(text) => text.as_bytes().to_vec(),
                Body::Empty => Vec::new(),
            };

            if body_bytes.is_empty() {
                return Ok(Response::builder()
                    .status(400)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&json!({
                        "error": "empty_file",
                        "message": "No file data provided. Use x-upload-mode: presigned for large files."
                    }))?))
                    .map_err(Box::new)?);
            }

            // For direct uploads, limit to 50MB (Lambda request size limit is ~6MB, but we'll be generous)
            if body_bytes.len() > 50 * 1024 * 1024 {
                return Ok(Response::builder()
                    .status(413)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&json!({
                        "error": "file_too_large",
                        "message": "File too large for direct upload. Use x-upload-mode: presigned for files > 50MB.",
                        "maxDirectSize": "50MB"
                    }))?))
                    .map_err(Box::new)?);
            }

            let _put_result = s3_client
                .put_object()
                .bucket(&bucket_name)
                .key(&s3_key)
                .body(aws_sdk_s3::primitives::ByteStream::from(body_bytes.clone()))
                .content_type(content_type)
                .send()
                .await
                .map_err(Box::new)?;

            (body_bytes.len() as u64, None)
        }
    };

    // Create metadata record in memories table
    let memories_table = std::env::var("MEMORIES_TABLE").unwrap_or_default();

    let mut item = HashMap::new();
    item.insert("memoryId".to_string(), AttributeValue::S(memory_id.clone()));
    item.insert("userId".to_string(), AttributeValue::S(user_id.to_string()));
    item.insert("s3Key".to_string(), AttributeValue::S(s3_key.clone()));
    item.insert(
        "s3Bucket".to_string(),
        AttributeValue::S(bucket_name.clone()),
    );
    item.insert(
        "contentType".to_string(),
        AttributeValue::S(content_type.to_string()),
    );
    item.insert(
        "createdAt".to_string(),
        AttributeValue::S(timestamp.to_rfc3339()),
    );

    // Set different status based on upload mode
    if upload_url.is_some() {
        // Pre-signed URL mode - file not uploaded yet
        item.insert(
            "status".to_string(),
            AttributeValue::S("pending_upload".to_string()),
        );
        item.insert("fileSize".to_string(), AttributeValue::N("0".to_string()));
    } else {
        // Direct upload mode - file already uploaded
        item.insert(
            "status".to_string(),
            AttributeValue::S("processing".to_string()),
        );
        item.insert(
            "fileSize".to_string(),
            AttributeValue::N(file_size.to_string()),
        );
    }

    let _put_item_result = dynamodb_client
        .put_item()
        .table_name(&memories_table)
        .set_item(Some(item))
        .send()
        .await
        .map_err(Box::new)?;

    // Trigger background processing only for direct uploads (pre-signed uploads will be triggered by S3 events)
    if upload_url.is_none() && file_size > 0 {
        info!("Triggering background processing for memory: {}", memory_id);

        let processing_message = json!({
            "memoryId": memory_id,
            "userId": user_id,
            "s3Bucket": bucket_name,
            "s3Key": s3_key,
            "fileSize": file_size,
            "uploadedAt": timestamp.to_rfc3339()
        });

        let message_body = serde_json::to_string(&processing_message)?;

        // Send to enrichment queue for AI processing (memory cards, facts, etc.)
        if let Ok(enrichment_queue_url) = std::env::var("ENRICHMENT_QUEUE_URL") {
            match sqs_client
                .send_message()
                .queue_url(&enrichment_queue_url)
                .message_body(&message_body)
                .send()
                .await
            {
                Ok(_) => info!("Sent message to enrichment queue for memory: {}", memory_id),
                Err(e) => error!("Failed to send enrichment message: {}", e),
            }
        }

        // Send to sketch builder queue for fast search indexing
        if let Ok(sketch_queue_url) = std::env::var("SKETCH_BUILDER_QUEUE_URL") {
            match sqs_client
                .send_message()
                .queue_url(&sketch_queue_url)
                .message_body(&message_body)
                .send()
                .await
            {
                Ok(_) => info!(
                    "Sent message to sketch builder queue for memory: {}",
                    memory_id
                ),
                Err(e) => error!("Failed to send sketch builder message: {}", e),
            }
        }
    }

    // Update usage tracking (only for completed uploads)
    if file_size > 0 {
        let usage_table = std::env::var("USAGE_TABLE").unwrap_or_default();
        let today = timestamp.format("%Y-%m-%d").to_string();

        let usage_key = HashMap::from([
            ("userId".to_string(), AttributeValue::S(user_id.to_string())),
            ("date".to_string(), AttributeValue::S(today)),
        ]);

        // Increment usage counter
        let _usage_result = dynamodb_client
            .update_item()
            .table_name(&usage_table)
            .set_key(Some(usage_key))
            .update_expression("ADD uploadCount :inc, bytesUploaded :bytes")
            .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
            .expression_attribute_values(":bytes", AttributeValue::N(file_size.to_string()))
            .send()
            .await
            .map_err(Box::new)?;
    }

    let mut body = json!({
        "memoryId": memory_id,
        "userId": user_id,
        "s3Key": s3_key,
        "fileSize": file_size,
        "createdAt": timestamp.to_rfc3339(),
    });

    if let Some(url) = upload_url {
        // Pre-signed URL response
        body["uploadUrl"] = json!(url);
        body["status"] = json!("pending_upload");
        body["message"] =
            json!("Pre-signed URL generated. Upload your .mv2 file to the provided URL.");
        body["instructions"] =
            json!("Make a PUT request to uploadUrl with your .mv2 file as the request body.");
    } else {
        // Direct upload response
        body["status"] = json!("processing");
        body["message"] = json!("Memory file uploaded successfully and is being processed.");
    }

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
