use aws_config::BehaviorVersion;
use aws_sdk_s3::Client as S3Client;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use shared::memvid::MemvidClient;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
struct SketchBuilderEvent {
    #[serde(rename = "Records")]
    records: Vec<EventRecord>,
}

#[derive(Debug, Deserialize)]
struct EventRecord {
    #[serde(rename = "eventSource")]
    _event_source: Option<String>,

    // For SQS messages
    body: Option<String>,

    // For DynamoDB Streams
    #[serde(rename = "eventName")]
    event_name: Option<String>,

    #[serde(rename = "dynamodb")]
    dynamodb: Option<DynamoDbStreamRecord>,
}

#[derive(Debug, Deserialize)]
struct DynamoDbStreamRecord {
    #[serde(rename = "NewImage")]
    new_image: Option<serde_json::Value>,
    #[serde(rename = "Keys")]
    _keys: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct SqsMessageBody {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "userId")]
    _user_id: Option<String>,
    #[serde(rename = "triggerType")]
    _trigger_type: Option<String>,
}

#[derive(Debug, Serialize)]
struct SketchBuilderResult {
    processed_count: i32,
    success_count: i32,
    error_count: i32,
    processing_duration_ms: i64,
    results: Vec<MemorySketchResult>,
}

#[derive(Debug, Serialize)]
struct MemorySketchResult {
    memory_id: String,
    success: bool,
    sketch_tracks_built: bool,
    processing_time_ms: i64,
    size_before_bytes: i64,
    size_after_bytes: i64,
    error_message: Option<String>,
}

async fn handler(event: LambdaEvent<SketchBuilderEvent>) -> Result<SketchBuilderResult, Error> {
    info!("Starting sketch builder process");

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);

    let bucket_name =
        std::env::var("MEMORY_BUCKET").map_err(|_| "MEMORY_BUCKET environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client.clone(), bucket_name.clone());

    let start_time = std::time::Instant::now();
    let mut results = Vec::new();
    let mut success_count = 0;
    let mut error_count = 0;

    // Process each record in the event
    for record in event.payload.records {
        let memory_ids = extract_memory_ids_from_record(&record)?;

        for memory_id in memory_ids {
            info!("Processing memory {} for sketch building", memory_id);

            let process_start = std::time::Instant::now();

            match build_sketch_tracks(&memvid_client, &s3_client, &memory_id, &bucket_name).await {
                Ok(result) => {
                    success_count += 1;
                    results.push(MemorySketchResult {
                        memory_id: memory_id.clone(),
                        success: true,
                        sketch_tracks_built: true,
                        processing_time_ms: process_start.elapsed().as_millis() as i64,
                        size_before_bytes: result.size_before,
                        size_after_bytes: result.size_after,
                        error_message: None,
                    });

                    info!("Successfully built sketch tracks for memory {}", memory_id);
                }
                Err(e) => {
                    error!(
                        "Failed to build sketch tracks for memory {}: {}",
                        memory_id, e
                    );
                    error_count += 1;
                    results.push(MemorySketchResult {
                        memory_id: memory_id.clone(),
                        success: false,
                        sketch_tracks_built: false,
                        processing_time_ms: process_start.elapsed().as_millis() as i64,
                        size_before_bytes: 0,
                        size_after_bytes: 0,
                        error_message: Some(e.to_string()),
                    });
                }
            }
        }
    }

    let processing_duration = start_time.elapsed();
    let processed_count = results.len() as i32;

    info!(
        "Sketch builder completed: processed={}, success={}, errors={}, duration_ms={}",
        processed_count,
        success_count,
        error_count,
        processing_duration.as_millis()
    );

    Ok(SketchBuilderResult {
        processed_count,
        success_count,
        error_count,
        processing_duration_ms: processing_duration.as_millis() as i64,
        results,
    })
}

fn extract_memory_ids_from_record(record: &EventRecord) -> Result<Vec<String>, Error> {
    let mut memory_ids = Vec::new();

    // Check if it's an SQS message
    if let Some(body) = &record.body {
        match serde_json::from_str::<SqsMessageBody>(body) {
            Ok(message) => {
                memory_ids.push(message.memory_id);
            }
            Err(e) => {
                warn!("Failed to parse SQS message body: {} - body: {}", e, body);
                // Try to extract memory_id directly if it's a simple string
                if let Ok(simple_message) = serde_json::from_str::<serde_json::Value>(body) {
                    if let Some(memory_id) = simple_message.get("memoryId").and_then(|v| v.as_str())
                    {
                        memory_ids.push(memory_id.to_string());
                    }
                }
            }
        }
    }

    // Check if it's a DynamoDB Stream record
    if let Some(dynamodb_record) = &record.dynamodb {
        if let Some(event_name) = &record.event_name {
            // Process INSERT and MODIFY events where status becomes 'active'
            if event_name == "INSERT" || event_name == "MODIFY" {
                if let Some(new_image) = &dynamodb_record.new_image {
                    // Check if this is a newly active memory
                    let status = new_image
                        .get("status")
                        .and_then(|v| v.get("S"))
                        .and_then(|v| v.as_str());

                    if status == Some("active") {
                        if let Some(memory_id) = new_image
                            .get("memoryId")
                            .and_then(|v| v.get("S"))
                            .and_then(|v| v.as_str())
                        {
                            memory_ids.push(memory_id.to_string());
                        }
                    }
                }
            }
        }
    }

    Ok(memory_ids)
}

struct SketchBuildResult {
    size_before: i64,
    size_after: i64,
}

async fn build_sketch_tracks(
    _memvid_client: &MemvidClient,
    s3_client: &S3Client,
    memory_id: &str,
    bucket_name: &str,
) -> Result<SketchBuildResult, Box<dyn std::error::Error + Send + Sync>> {
    let s3_key = format!("memories/{}.mv2", memory_id);

    // Download current .mv2 file
    let obj = s3_client
        .get_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .send()
        .await?;

    let original_size = obj.content_length().unwrap_or(0);
    let data = obj.body.collect().await?.into_bytes();

    // Save to temporary file
    let mut temp_file = NamedTempFile::new()?;
    std::io::Write::write_all(&mut temp_file, &data)?;
    temp_file.flush()?;

    // Create output temporary file
    let output_temp = NamedTempFile::new()?;

    // Run memvid sketch build
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = Command::new(memvid_path)
        .arg("sketch")
        .arg("build")
        .arg("--output")
        .arg(output_temp.path())
        .arg(temp_file.path())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("memvid sketch build failed: {}", stderr).into());
    }

    // Get final file size
    let final_size = std::fs::metadata(output_temp.path())?.len() as i64;

    // Re-upload the file with sketch tracks to S3
    let sketch_data = std::fs::read(output_temp.path())?;

    s3_client
        .put_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .body(sketch_data.into())
        .content_type("application/octet-stream")
        .metadata("sketch-tracks-built", "true")
        .send()
        .await?;

    info!(
        "Sketch tracks built for memory {}: {} bytes → {} bytes",
        memory_id, original_size, final_size
    );

    Ok(SketchBuildResult {
        size_before: original_size,
        size_after: final_size,
    })
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    lambda_runtime::run(service_fn(handler)).await
}
