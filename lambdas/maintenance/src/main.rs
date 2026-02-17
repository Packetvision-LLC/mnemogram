use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DynamoDbClient;
use aws_sdk_s3::Client as S3Client;
use chrono::{DateTime, Utc};
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use shared::memvid::MemvidClient;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
struct MaintenanceEvent {
    // CloudWatch Events schedule - can be empty
    #[serde(default)]
    #[allow(dead_code)]
    source: String,
}

#[derive(Debug, Serialize)]
struct MaintenanceResult {
    processed_count: i32,
    skipped_count: i32,
    error_count: i32,
    total_frames_reclaimed: i64,
    total_space_saved_bytes: i64,
    processing_duration_ms: i64,
}

#[derive(Debug, Serialize)]
struct MemoryProcessResult {
    memory_id: String,
    success: bool,
    frames_before: i64,
    frames_after: i64,
    frames_reclaimed: i64,
    size_before_bytes: i64,
    size_after_bytes: i64,
    space_saved_bytes: i64,
    processing_time_ms: i64,
    error_message: Option<String>,
}

async fn handler(_event: LambdaEvent<MaintenanceEvent>) -> Result<MaintenanceResult, Error> {
    info!("Starting maintenance vacuum/compaction process");

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamo_client = DynamoDbClient::new(&config);
    let s3_client = S3Client::new(&config);

    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

    let bucket_name =
        std::env::var("MEMORY_BUCKET").map_err(|_| "MEMORY_BUCKET environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client.clone(), bucket_name.clone());

    let start_time = std::time::Instant::now();

    // Scan memories table for all active memories
    let mut scan_result = dynamo_client
        .scan()
        .table_name(&memories_table)
        .filter_expression("#status = :status")
        .expression_attribute_names("#status", "status")
        .expression_attribute_values(":status", AttributeValue::S("active".to_string()))
        .send()
        .await
        .map_err(|e| format!("Failed to scan memories table: {}", e))?;

    let mut results = Vec::new();
    let mut processed_count = 0;
    let mut skipped_count = 0;
    let mut error_count = 0;
    let mut total_frames_reclaimed = 0i64;
    let mut total_space_saved_bytes = 0i64;

    // Get current time for comparison
    let now = Utc::now();
    let seven_days_ago = now - chrono::Duration::days(7);

    loop {
        let items = scan_result.items();

        for item in items {
            let memory_id: String = item
                .get("memoryId")
                .and_then(|v: &AttributeValue| v.as_s().ok())
                .cloned()
                .unwrap_or_default();

            if memory_id.is_empty() {
                continue;
            }

            // Check lastVacuumedAt to see if we need to process this memory
            let should_vacuum = match item.get("lastVacuumedAt") {
                Some(AttributeValue::S(last_vacuumed_str)) => {
                    match DateTime::parse_from_rfc3339(last_vacuumed_str) {
                        Ok(last_vacuumed) => {
                            let last_vacuumed_utc = last_vacuumed.with_timezone(&Utc);
                            last_vacuumed_utc < seven_days_ago
                        }
                        Err(_) => {
                            warn!(
                                "Invalid lastVacuumedAt timestamp for memory {}: {}",
                                memory_id, last_vacuumed_str
                            );
                            true // Vacuum if timestamp is invalid
                        }
                    }
                }
                _ => true, // Vacuum if no lastVacuumedAt field
            };

            if !should_vacuum {
                info!("Skipping memory {} - vacuumed recently", memory_id);
                skipped_count += 1;
                continue;
            }

            info!("Processing memory {} for vacuum/compaction", memory_id);

            match process_memory(&memvid_client, &s3_client, &memory_id, &bucket_name).await {
                Ok(result) => {
                    // Update lastVacuumedAt in DynamoDB
                    if let Err(e) =
                        update_last_vacuumed(&dynamo_client, &memories_table, &memory_id).await
                    {
                        warn!(
                            "Failed to update lastVacuumed timestamp for {}: {}",
                            memory_id, e
                        );
                    }

                    total_frames_reclaimed += result.frames_reclaimed;
                    total_space_saved_bytes += result.space_saved_bytes;
                    processed_count += 1;
                    results.push(result);
                }
                Err(e) => {
                    error!("Failed to process memory {}: {}", memory_id, e);
                    error_count += 1;
                    results.push(MemoryProcessResult {
                        memory_id,
                        success: false,
                        frames_before: 0,
                        frames_after: 0,
                        frames_reclaimed: 0,
                        size_before_bytes: 0,
                        size_after_bytes: 0,
                        space_saved_bytes: 0,
                        processing_time_ms: 0,
                        error_message: Some(e.to_string()),
                    });
                }
            }
        }

        // Check if there are more items to scan
        if let Some(last_key) = scan_result.last_evaluated_key() {
            scan_result = dynamo_client
                .scan()
                .table_name(&memories_table)
                .filter_expression("#status = :status")
                .expression_attribute_names("#status", "status")
                .expression_attribute_values(":status", AttributeValue::S("active".to_string()))
                .set_exclusive_start_key(Some(last_key.clone()))
                .send()
                .await
                .map_err(|e| format!("Failed to continue scan: {}", e))?;
        } else {
            break;
        }
    }

    let processing_duration = start_time.elapsed();

    // Log detailed results
    info!("Maintenance completed: processed={}, skipped={}, errors={}, frames_reclaimed={}, space_saved_mb={:.2}, duration_ms={}",
          processed_count, skipped_count, error_count, total_frames_reclaimed,
          total_space_saved_bytes as f64 / (1024.0 * 1024.0), processing_duration.as_millis());

    for result in &results {
        if result.success {
            info!("Memory {} - frames: {} → {} (reclaimed {}), size: {:.2}MB → {:.2}MB (saved {:.2}MB)",
                  result.memory_id,
                  result.frames_before, result.frames_after, result.frames_reclaimed,
                  result.size_before_bytes as f64 / (1024.0 * 1024.0),
                  result.size_after_bytes as f64 / (1024.0 * 1024.0),
                  result.space_saved_bytes as f64 / (1024.0 * 1024.0));
        } else {
            error!(
                "Memory {} failed: {}",
                result.memory_id,
                result.error_message.as_deref().unwrap_or("Unknown error")
            );
        }
    }

    Ok(MaintenanceResult {
        processed_count,
        skipped_count,
        error_count,
        total_frames_reclaimed,
        total_space_saved_bytes,
        processing_duration_ms: processing_duration.as_millis() as i64,
    })
}

async fn process_memory(
    _memvid_client: &MemvidClient,
    s3_client: &S3Client,
    memory_id: &str,
    bucket_name: &str,
) -> Result<MemoryProcessResult, Box<dyn std::error::Error + Send + Sync>> {
    let process_start = std::time::Instant::now();

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

    // Get stats before processing
    let stats_before = get_memvid_stats(temp_file.path())?;

    // Create output temporary file
    let output_temp = NamedTempFile::new()?;

    // Run memvid doctor with vacuum and index rebuild
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = Command::new(memvid_path)
        .arg("doctor")
        .arg("--vacuum")
        .arg("--rebuild-lex-index")
        .arg("--rebuild-time-index")
        .arg("--output")
        .arg(output_temp.path())
        .arg(temp_file.path())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("memvid doctor failed: {}", stderr).into());
    }

    // Get stats after processing
    let stats_after = get_memvid_stats(output_temp.path())?;
    let optimized_size = std::fs::metadata(output_temp.path())?.len() as i64;

    // Re-upload the optimized file to S3
    let optimized_data = std::fs::read(output_temp.path())?;

    s3_client
        .put_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .body(optimized_data.into())
        .content_type("application/octet-stream")
        .send()
        .await?;

    let processing_time = process_start.elapsed();

    Ok(MemoryProcessResult {
        memory_id: memory_id.to_string(),
        success: true,
        frames_before: stats_before.frame_count,
        frames_after: stats_after.frame_count,
        frames_reclaimed: stats_before.frame_count - stats_after.frame_count,
        size_before_bytes: original_size,
        size_after_bytes: optimized_size,
        space_saved_bytes: original_size - optimized_size,
        processing_time_ms: processing_time.as_millis() as i64,
        error_message: None,
    })
}

#[derive(Debug)]
struct MemvidStats {
    frame_count: i64,
}

fn get_memvid_stats(
    file_path: &Path,
) -> Result<MemvidStats, Box<dyn std::error::Error + Send + Sync>> {
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = std::process::Command::new(memvid_path)
        .arg("stats")
        .arg("--json")
        .arg(file_path)
        .output()?;

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

async fn update_last_vacuumed(
    dynamo_client: &DynamoDbClient,
    table_name: &str,
    memory_id: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let now = Utc::now().to_rfc3339();

    dynamo_client
        .update_item()
        .table_name(table_name)
        .key("memoryId", AttributeValue::S(memory_id.to_string()))
        .update_expression("SET lastVacuumedAt = :timestamp")
        .expression_attribute_values(":timestamp", AttributeValue::S(now))
        .send()
        .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    lambda_runtime::run(service_fn(handler)).await
}
