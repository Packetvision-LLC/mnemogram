use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_sqs::Client as SqsClient;
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::memvid::MemvidClient;
use shared::errors::MnemogramError;
use std::collections::HashMap;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Deserialize)]
struct S3Event {
    #[serde(rename = "Records")]
    records: Vec<S3Record>,
}

#[derive(Debug, Deserialize)]
struct S3Record {
    #[serde(rename = "eventName")]
    event_name: String,
    s3: S3Info,
}

#[derive(Debug, Deserialize)]
struct S3Info {
    bucket: S3Bucket,
    object: S3Object,
}

#[derive(Debug, Deserialize)]
struct S3Bucket {
    name: String,
}

#[derive(Debug, Deserialize)]
struct S3Object {
    key: String,
    size: u64,
}

#[derive(Debug, Serialize)]
struct IndexRebuildMessage {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "frameCount")]
    frame_count: i64,
    #[serde(rename = "triggerType")]
    trigger_type: String,
}

#[derive(Debug)]
struct MemvidStats {
    frame_count: i64,
}

/// Lambda function triggered by S3 PUT events to validate uploaded .mv2 files
async fn function_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    let sqs_client = SqsClient::new(&config);
    
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

    // Optional SQS queue for triggering index rebuilds
    let index_rebuild_queue_url = std::env::var("INDEX_REBUILD_QUEUE_URL").ok();

    let memvid_client = MemvidClient::new(s3_client.clone(), "".to_string()); // bucket will be set per record

    for record in event.payload.records {
        if !record.event_name.starts_with("ObjectCreated") {
            continue;
        }

        let bucket = record.s3.bucket.name;
        let key = record.s3.object.key;
        let size = record.s3.object.size;

        tracing::info!("Processing uploaded file: s3://{}/{} ({} bytes)", bucket, key, size);

        // Extract memory ID from S3 key (format: memories/{memory_id}.mv2)
        let memory_id = if let Some(captures) = regex::Regex::new(r"memories/([^/]+)\.mv2$")
            .unwrap()
            .captures(&key)
        {
            captures.get(1).unwrap().as_str().to_string()
        } else {
            tracing::warn!("S3 key {} doesn't match expected format memories/{{memory_id}}.mv2", key);
            continue;
        };

        // Create a new memvid client for this bucket
        let bucket_specific_client = MemvidClient::new(s3_client.clone(), bucket.clone());

        // Get frame count and validate the .mv2 file
        let frame_count_result = get_frame_count(&s3_client, &bucket, &key).await;
        let new_frame_count = frame_count_result.unwrap_or(0);

        // Get the previous frame count from DynamoDB
        let previous_frame_count = get_previous_frame_count(&dynamodb_client, &memories_table, &memory_id).await.unwrap_or(0);

        // Validate the .mv2 file
        let validation_result = match bucket_specific_client.validate_mv2_file(&memory_id).await {
            Ok(is_valid) => {
                if is_valid {
                    tracing::info!("Successfully validated .mv2 file for memory {} - {} frames", memory_id, new_frame_count);
                    "valid"
                } else {
                    tracing::error!("Invalid .mv2 file format for memory {}", memory_id);
                    "invalid"
                }
            }
            Err(MnemogramError::S3Error(msg)) => {
                tracing::error!("Failed to validate .mv2 file for memory {}: {}", memory_id, msg);
                "error"
            }
            Err(e) => {
                tracing::error!("Unexpected error validating memory {}: {:?}", memory_id, e);
                "error"
            }
        };

        // Update the memory record in DynamoDB
        let status = match validation_result {
            "valid" => "ready", // Memory is ready for search
            "invalid" => "invalid_format",
            _ => "validation_error",
        };

        let key_attrs = HashMap::from([
            ("memoryId".to_string(), AttributeValue::S(memory_id.clone()))
        ]);

        let mut update_expression = "SET #status = :status, #sizeBytes = :sizeBytes, #updatedAt = :updatedAt, #frameCount = :frameCount".to_string();
        let mut expression_attribute_names = HashMap::new();
        let mut expression_attribute_values = HashMap::new();

        expression_attribute_names.insert("#status".to_string(), "status".to_string());
        expression_attribute_names.insert("#sizeBytes".to_string(), "sizeBytes".to_string());
        expression_attribute_names.insert("#updatedAt".to_string(), "updatedAt".to_string());
        expression_attribute_names.insert("#frameCount".to_string(), "frameCount".to_string());

        expression_attribute_values.insert(":status".to_string(), AttributeValue::S(status.to_string()));
        expression_attribute_values.insert(":sizeBytes".to_string(), AttributeValue::N(size.to_string()));
        expression_attribute_values.insert(":updatedAt".to_string(), AttributeValue::S(chrono::Utc::now().to_rfc3339()));
        expression_attribute_values.insert(":frameCount".to_string(), AttributeValue::N(new_frame_count.to_string()));

        if validation_result != "valid" {
            update_expression.push_str(", #validationError = :validationError");
            expression_attribute_names.insert("#validationError".to_string(), "validationError".to_string());
            
            let error_msg = match validation_result {
                "invalid" => "File format is not a valid .mv2 memory file",
                _ => "Failed to validate memory file",
            };
            expression_attribute_values.insert(":validationError".to_string(), AttributeValue::S(error_msg.to_string()));
        }

        let update_result = dynamodb_client
            .update_item()
            .table_name(&memories_table)
            .set_key(Some(key_attrs))
            .update_expression(update_expression)
            .set_expression_attribute_names(Some(expression_attribute_names))
            .set_expression_attribute_values(Some(expression_attribute_values))
            .send()
            .await;

        match update_result {
            Ok(_) => {
                tracing::info!("Updated memory {} status to {} with {} frames", memory_id, status, new_frame_count);
                
                // Check if we should trigger an index rebuild (MNEM-158)
                if validation_result == "valid" && 
                   new_frame_count > previous_frame_count + 100 &&
                   index_rebuild_queue_url.is_some() 
                {
                    let frame_increase = new_frame_count - previous_frame_count;
                    tracing::info!("Memory {} frame count increased by {} ({}→{}), triggering index rebuild", 
                                   memory_id, frame_increase, previous_frame_count, new_frame_count);
                    
                    if let Err(e) = trigger_index_rebuild(&sqs_client, 
                                                         index_rebuild_queue_url.as_ref().unwrap(), 
                                                         &memory_id, 
                                                         new_frame_count).await {
                        tracing::error!("Failed to trigger index rebuild for memory {}: {}", memory_id, e);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to update memory {} status: {:?}", memory_id, e);
            }
        }
    }

    Ok(())
}

async fn get_frame_count(s3_client: &S3Client, bucket: &str, key: &str) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    // Download the .mv2 file to get frame count
    let obj = s3_client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await?;

    let data = obj.body.collect().await?.into_bytes();

    // Save to temporary file
    let mut temp_file = NamedTempFile::new()?;
    std::io::Write::write_all(&mut temp_file, &data)?;
    temp_file.flush()?;

    // Get stats using memvid CLI
    let stats = get_memvid_stats(temp_file.path()).await?;
    Ok(stats.frame_count)
}

async fn get_memvid_stats(file_path: &Path) -> Result<MemvidStats, Box<dyn std::error::Error + Send + Sync>> {
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = Command::new(memvid_path)
        .arg("stats")
        .arg("--json")
        .arg(file_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("memvid stats failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stats: serde_json::Value = serde_json::from_str(&stdout)?;
    
    let frame_count = stats
        .get("frame_count")
        .or_else(|| stats.get("frames"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    Ok(MemvidStats { frame_count })
}

async fn get_previous_frame_count(
    dynamodb_client: &aws_sdk_dynamodb::Client, 
    table_name: &str, 
    memory_id: &str
) -> Result<i64, Box<dyn std::error::Error + Send + Sync>> {
    let result = dynamodb_client
        .get_item()
        .table_name(table_name)
        .key("memoryId", AttributeValue::S(memory_id.to_string()))
        .projection_expression("frameCount")
        .send()
        .await?;

    let frame_count = result.item()
        .and_then(|item| item.get("frameCount"))
        .and_then(|attr| attr.as_n().ok())
        .and_then(|n| n.parse::<i64>().ok())
        .unwrap_or(0);

    Ok(frame_count)
}

async fn trigger_index_rebuild(
    sqs_client: &SqsClient,
    queue_url: &str,
    memory_id: &str,
    frame_count: i64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let message = IndexRebuildMessage {
        memory_id: memory_id.to_string(),
        frame_count,
        trigger_type: "frame_threshold".to_string(),
    };

    let message_body = serde_json::to_string(&message)?;

    sqs_client
        .send_message()
        .queue_url(queue_url)
        .message_body(message_body)
        .send()
        .await?;

    tracing::info!("Sent index rebuild message for memory {} to SQS queue", memory_id);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(function_handler)).await
}