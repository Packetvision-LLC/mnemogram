use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shared::memvid::MemvidClient;
use shared::errors::MnemogramError;
use std::collections::HashMap;
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

/// Lambda function triggered by S3 PUT events to validate uploaded .mv2 files
async fn function_handler(event: LambdaEvent<S3Event>) -> Result<(), Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

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

        // Validate the .mv2 file
        let validation_result = match bucket_specific_client.validate_mv2_file(&memory_id).await {
            Ok(is_valid) => {
                if is_valid {
                    tracing::info!("Successfully validated .mv2 file for memory {}", memory_id);
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

        let mut update_expression = "SET #status = :status, #sizeBytes = :sizeBytes, #updatedAt = :updatedAt".to_string();
        let mut expression_attribute_names = HashMap::new();
        let mut expression_attribute_values = HashMap::new();

        expression_attribute_names.insert("#status".to_string(), "status".to_string());
        expression_attribute_names.insert("#sizeBytes".to_string(), "sizeBytes".to_string());
        expression_attribute_names.insert("#updatedAt".to_string(), "updatedAt".to_string());

        expression_attribute_values.insert(":status".to_string(), AttributeValue::S(status.to_string()));
        expression_attribute_values.insert(":sizeBytes".to_string(), AttributeValue::N(size.to_string()));
        expression_attribute_values.insert(":updatedAt".to_string(), AttributeValue::S(chrono::Utc::now().to_rfc3339()));

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
                tracing::info!("Updated memory {} status to {}", memory_id, status);
            }
            Err(e) => {
                tracing::error!("Failed to update memory {} status: {:?}", memory_id, e);
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

    run(service_fn(function_handler)).await
}