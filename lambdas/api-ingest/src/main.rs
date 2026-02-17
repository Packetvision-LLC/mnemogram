use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3 as s3;
use aws_sdk_sqs;
use chrono::Utc;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::json;
use std::collections::HashMap;
use tracing::{error, info, warn};
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

    // Update ingest threshold tracking (only for completed uploads)
    if file_size > 0 {
        if let Err(e) = update_ingest_threshold_tracking(&dynamodb_client, &user_id, &timestamp).await {
            warn!("Failed to update ingest threshold tracking: {}", e);
            // Don't fail the ingest for threshold tracking errors
        }
    }

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

        // Check if we should trigger index rebuild based on thresholds
        if let Err(e) = check_and_trigger_index_rebuild(&dynamodb_client, &sqs_client, &user_id).await {
            warn!("Failed to check index rebuild threshold: {}", e);
            // Don't fail the ingest for threshold check errors
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

/// Update ingest threshold tracking for a user
async fn update_ingest_threshold_tracking(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    user_id: &str,
    timestamp: &chrono::DateTime<chrono::Utc>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let threshold_table = std::env::var("THRESHOLD_TABLE")
        .unwrap_or_else(|_| "mnemogram-dev-threshold-tracking".to_string());

    let today = timestamp.format("%Y-%m-%d").to_string();
    let this_hour = timestamp.format("%Y-%m-%d-%H").to_string();

    // Update daily ingest count for user
    let user_daily_key = HashMap::from([
        ("pk".to_string(), AttributeValue::S(format!("user#{}", user_id))),
        ("sk".to_string(), AttributeValue::S(format!("daily#{}", today))),
    ]);

    let _daily_result = dynamodb_client
        .update_item()
        .table_name(&threshold_table)
        .set_key(Some(user_daily_key))
        .update_expression("ADD ingestCount :inc SET lastUpdated = :timestamp")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":timestamp", AttributeValue::S(timestamp.to_rfc3339()))
        .send()
        .await?;

    // Update hourly ingest count for user (for more fine-grained thresholds)
    let user_hourly_key = HashMap::from([
        ("pk".to_string(), AttributeValue::S(format!("user#{}", user_id))),
        ("sk".to_string(), AttributeValue::S(format!("hourly#{}", this_hour))),
    ]);

    let _hourly_result = dynamodb_client
        .update_item()
        .table_name(&threshold_table)
        .set_key(Some(user_hourly_key))
        .update_expression("ADD ingestCount :inc SET lastUpdated = :timestamp, #ttl = :ttl")
        .expression_attribute_names("#ttl", "ttl")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":timestamp", AttributeValue::S(timestamp.to_rfc3339()))
        .expression_attribute_values(":ttl", AttributeValue::N((timestamp.timestamp() + 86400 * 7).to_string())) // 7 day TTL
        .send()
        .await?;

    // Update global daily counters
    let global_daily_key = HashMap::from([
        ("pk".to_string(), AttributeValue::S("global".to_string())),
        ("sk".to_string(), AttributeValue::S(format!("daily#{}", today))),
    ]);

    let _global_result = dynamodb_client
        .update_item()
        .table_name(&threshold_table)
        .set_key(Some(global_daily_key))
        .update_expression("ADD ingestCount :inc, uniqueUsers :user SET lastUpdated = :timestamp")
        .expression_attribute_values(":inc", AttributeValue::N("1".to_string()))
        .expression_attribute_values(":user", AttributeValue::SS(vec![user_id.to_string()]))
        .expression_attribute_values(":timestamp", AttributeValue::S(timestamp.to_rfc3339()))
        .send()
        .await?;

    Ok(())
}

/// Check if we should trigger index rebuild based on thresholds and trigger if needed
async fn check_and_trigger_index_rebuild(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    sqs_client: &aws_sdk_sqs::Client,
    user_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let threshold_table = std::env::var("THRESHOLD_TABLE")
        .unwrap_or_else(|_| "mnemogram-dev-threshold-tracking".to_string());

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    
    // Check user's daily ingest count
    let user_daily_key = HashMap::from([
        ("pk".to_string(), AttributeValue::S(format!("user#{}", user_id))),
        ("sk".to_string(), AttributeValue::S(format!("daily#{}", today))),
    ]);

    let user_result = dynamodb_client
        .get_item()
        .table_name(&threshold_table)
        .set_key(Some(user_daily_key))
        .send()
        .await?;

    if let Some(item) = user_result.item() {
        let daily_count = item
            .get("ingestCount")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        // Define threshold values (configurable via environment)
        let user_daily_threshold = std::env::var("USER_DAILY_REBUILD_THRESHOLD")
            .unwrap_or_else(|_| "10".to_string())
            .parse::<u32>()
            .unwrap_or(10);

        // Check if user has hit the threshold and needs index rebuild
        if daily_count >= user_daily_threshold && daily_count % user_daily_threshold == 0 {
            info!(
                "User {} hit daily ingest threshold ({} ingests), triggering index rebuild",
                user_id, daily_count
            );

            // Send message to maintenance queue for user-specific rebuild
            if let Ok(maintenance_queue_url) = std::env::var("MAINTENANCE_QUEUE_URL") {
                let rebuild_message = json!({
                    "userId": user_id,
                    "triggerType": "threshold",
                    "threshold": "daily_ingest",
                    "count": daily_count,
                    "triggeredAt": chrono::Utc::now().to_rfc3339()
                });

                let message_body = serde_json::to_string(&rebuild_message)?;

                match sqs_client
                    .send_message()
                    .queue_url(&maintenance_queue_url)
                    .message_body(message_body)
                    .send()
                    .await
                {
                    Ok(_) => info!("Triggered index rebuild for user: {}", user_id),
                    Err(e) => error!("Failed to send index rebuild message: {}", e),
                }
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}
