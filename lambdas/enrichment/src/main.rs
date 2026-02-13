use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_s3::Client as S3Client;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use shared::memvid::MemvidClient;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{info, warn, error};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct EnrichmentEvent {
    #[serde(rename = "Records")]
    records: Vec<EventRecord>,
}

#[derive(Debug, Deserialize)]
struct EventRecord {
    #[serde(rename = "eventSource")]
    _event_source: Option<String>,
    
    // For SQS messages
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SqsMessageBody {
    #[serde(rename = "memoryId")]
    memory_id: String,
    #[serde(rename = "userId")]
    user_id: String,
    #[serde(rename = "subscriptionTier")]
    subscription_tier: Option<String>,
}

#[derive(Debug, Serialize)]
struct EnrichmentResult {
    processed_count: i32,
    success_count: i32,
    error_count: i32,
    processing_duration_ms: i64,
    results: Vec<MemoryEnrichmentResult>,
}

#[derive(Debug, Serialize)]
struct MemoryEnrichmentResult {
    memory_id: String,
    success: bool,
    enrichment_engine_used: String,
    processing_time_ms: i64,
    size_before_bytes: i64,
    size_after_bytes: i64,
    cards_extracted: Option<i32>,
    facts_extracted: Option<i32>,
    error_message: Option<String>,
}

async fn handler(event: LambdaEvent<EnrichmentEvent>) -> Result<EnrichmentResult, Error> {
    info!("Starting enrichment process");

    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = S3Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    let bucket_name = std::env::var("MEMORY_BUCKET")
        .map_err(|_| "MEMORY_BUCKET environment variable not set")?;
    
    let _subscriptions_table = std::env::var("SUBSCRIPTIONS_TABLE")
        .map_err(|_| "SUBSCRIPTIONS_TABLE environment variable not set")?;

    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

    let memvid_client = MemvidClient::new(s3_client.clone(), bucket_name.clone());

    let start_time = std::time::Instant::now();
    let mut results = Vec::new();
    let mut success_count = 0;
    let mut error_count = 0;

    // Process each record in the event
    for record in event.payload.records {
        let enrichment_requests = extract_enrichment_requests_from_record(&record)?;
        
        for sqs_msg in enrichment_requests {
            let req = EnrichmentRequest::from(sqs_msg);
            info!("Processing memory {} for enrichment", req.memory_id);

            // Update enrichment status to 'processing'
            if let Err(e) = update_memory_status(&dynamodb_client, &memories_table, &req.memory_id, "processing").await {
                warn!("Failed to update memory status to processing: {}", e);
            }

            let process_start = std::time::Instant::now();

            match enrich_memory(&memvid_client, &s3_client, &req, &bucket_name).await {
                Ok(result) => {
                    success_count += 1;
                    results.push(result);
                    
                    // Update enrichment status to 'complete'
                    if let Err(e) = update_memory_status(&dynamodb_client, &memories_table, &req.memory_id, "complete").await {
                        warn!("Failed to update memory status to complete: {}", e);
                    }
                    
                    info!("Successfully enriched memory {}", req.memory_id);
                }
                Err(e) => {
                    error!("Failed to enrich memory {}: {}", req.memory_id, e);
                    error_count += 1;
                    
                    results.push(MemoryEnrichmentResult {
                        memory_id: req.memory_id.clone(),
                        success: false,
                        enrichment_engine_used: "none".to_string(),
                        processing_time_ms: process_start.elapsed().as_millis() as i64,
                        size_before_bytes: 0,
                        size_after_bytes: 0,
                        cards_extracted: None,
                        facts_extracted: None,
                        error_message: Some(e.to_string()),
                    });
                    
                    // Update enrichment status to 'failed'
                    if let Err(e) = update_memory_status(&dynamodb_client, &memories_table, &req.memory_id, "failed").await {
                        warn!("Failed to update memory status to failed: {}", e);
                    }
                }
            }
        }
    }

    let processing_duration = start_time.elapsed();
    let processed_count = results.len() as i32;

    info!("Enrichment completed: processed={}, success={}, errors={}, duration_ms={}",
          processed_count, success_count, error_count, processing_duration.as_millis());

    Ok(EnrichmentResult {
        processed_count,
        success_count,
        error_count,
        processing_duration_ms: processing_duration.as_millis() as i64,
        results,
    })
}

fn extract_enrichment_requests_from_record(record: &EventRecord) -> Result<Vec<SqsMessageBody>, Error> {
    let mut requests = Vec::new();

    // Check if it's an SQS message
    if let Some(body) = &record.body {
        match serde_json::from_str::<SqsMessageBody>(body) {
            Ok(message) => {
                requests.push(message);
            }
            Err(e) => {
                warn!("Failed to parse SQS message body: {} - body: {}", e, body);
                // Try to extract minimal required fields
                if let Ok(simple_message) = serde_json::from_str::<serde_json::Value>(body) {
                    if let (Some(memory_id), Some(user_id)) = (
                        simple_message.get("memoryId").and_then(|v| v.as_str()),
                        simple_message.get("userId").and_then(|v| v.as_str())
                    ) {
                        requests.push(SqsMessageBody {
                            memory_id: memory_id.to_string(),
                            user_id: user_id.to_string(),
                            subscription_tier: simple_message.get("subscriptionTier").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        });
                    }
                }
            }
        }
    }

    Ok(requests)
}

async fn update_memory_status(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    table_name: &str,
    memory_id: &str,
    status: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let update_expression = "SET enrichmentStatus = :status, enrichmentLastUpdated = :timestamp";
    let mut expression_attribute_values = HashMap::new();
    expression_attribute_values.insert(":status".to_string(), AttributeValue::S(status.to_string()));
    expression_attribute_values.insert(":timestamp".to_string(), AttributeValue::N(chrono::Utc::now().timestamp().to_string()));

    dynamodb_client
        .update_item()
        .table_name(table_name)
        .key("memoryId", AttributeValue::S(memory_id.to_string()))
        .update_expression(update_expression)
        .set_expression_attribute_values(Some(expression_attribute_values))
        .send()
        .await?;

    Ok(())
}

struct EnrichmentRequest {
    memory_id: String,
    _user_id: String,
    subscription_tier: Option<String>,
}

impl From<SqsMessageBody> for EnrichmentRequest {
    fn from(body: SqsMessageBody) -> Self {
        EnrichmentRequest {
            memory_id: body.memory_id,
            _user_id: body.user_id,
            subscription_tier: body.subscription_tier,
        }
    }
}

async fn enrich_memory(
    _memvid_client: &MemvidClient,
    s3_client: &S3Client,
    request: &EnrichmentRequest,
    bucket_name: &str,
) -> Result<MemoryEnrichmentResult, Box<dyn std::error::Error + Send + Sync>> {
    let s3_key = format!("memories/{}.mv2", request.memory_id);

    // Download current .mv2 file
    let obj = s3_client
        .get_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .send()
        .await?;

    let original_size = obj.content_length().unwrap_or(0) as i64;
    let data = obj.body.collect().await?.into_bytes();

    // Save to temporary file
    let mut temp_file = NamedTempFile::new()?;
    std::io::Write::write_all(&mut temp_file, &data)?;
    temp_file.flush()?;

    // Create output temporary file
    let output_temp = NamedTempFile::new()?;

    // Determine which enrichment engine to use
    let is_premium = matches!(
        request.subscription_tier.as_deref(),
        Some("pro") | Some("enterprise") | Some("premium")
    );

    let engine = if is_premium {
        "claude"  // Use Claude API for pro/enterprise users
    } else {
        "rules"   // Use free rules-based engine
    };

    let process_start = std::time::Instant::now();

    // Run memvid enrich
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let mut command = Command::new(memvid_path);
    command
        .arg("enrich")
        .arg("--engine")
        .arg(engine)
        .arg("--output")
        .arg(output_temp.path())
        .arg(temp_file.path());

    // Set environment variables for Claude API if using premium engine
    if engine == "claude" {
        if let Ok(anthropic_api_key) = std::env::var("ANTHROPIC_API_KEY") {
            command.env("ANTHROPIC_API_KEY", anthropic_api_key);
        }
    }

    let output = command.output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("memvid enrich failed: {}", stderr).into());
    }

    // Get final file size
    let final_size = std::fs::metadata(output_temp.path())?.len() as i64;

    // Parse enrichment statistics from stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    let cards_extracted = extract_stat_from_output(&stdout, "cards");
    let facts_extracted = extract_stat_from_output(&stdout, "facts");

    // Re-upload the enriched file to S3
    let enriched_data = std::fs::read(output_temp.path())?;
    
    s3_client
        .put_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .body(enriched_data.into())
        .content_type("application/octet-stream")
        .metadata("enrichment-engine", engine)
        .metadata("enrichment-timestamp", &chrono::Utc::now().timestamp().to_string())
        .send()
        .await?;

    let processing_time = process_start.elapsed();

    info!("Memory {} enriched with {} engine: {} bytes → {} bytes, {} cards, {} facts", 
          request.memory_id, engine, original_size, final_size, 
          cards_extracted.unwrap_or(0), facts_extracted.unwrap_or(0));

    Ok(MemoryEnrichmentResult {
        memory_id: request.memory_id.clone(),
        success: true,
        enrichment_engine_used: engine.to_string(),
        processing_time_ms: processing_time.as_millis() as i64,
        size_before_bytes: original_size,
        size_after_bytes: final_size,
        cards_extracted,
        facts_extracted,
        error_message: None,
    })
}

fn extract_stat_from_output(output: &str, stat_name: &str) -> Option<i32> {
    // Look for patterns like "Extracted 42 cards" or "Found 17 facts"
    let patterns = [
        format!("Extracted {} {}", r"\d+", stat_name),
        format!("Found {} {}", r"\d+", stat_name),
        format!("{}: {}", stat_name, r"\d+"),
    ];
    
    for pattern in &patterns {
        if let Some(captures) = regex::Regex::new(pattern).ok()?.captures(output) {
            if let Some(num_str) = captures.get(0) {
                if let Ok(num) = num_str.as_str().parse::<i32>() {
                    return Some(num);
                }
            }
        }
    }
    
    None
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    lambda_runtime::run(service_fn(handler)).await
}