use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use lambda_http::{run, service_fn, Body, Error, Request, RequestExt, Response};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

mod extract_user_id;

/// GET /v1/memories/{id}/cards — Returns extracted memory cards from an enriched memory
/// Uses: memvid memories --json
/// Only available for Pro and Enterprise tiers
async fn handler(event: Request) -> Result<Response<Body>, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let s3_client = aws_sdk_s3::Client::new(&config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    // Extract path parameter
    let path_params = event.path_parameters();
    let memory_id = match path_params.first("id") {
        Some(id) => id,
        None => {
            return Ok(Response::builder()
                .status(400)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "missing_memory_id",
                    "message": "Memory ID is required in path"
                }))?))
                .map_err(Box::new)?);
        }
    };

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

    let bucket_name =
        std::env::var("MEMORY_BUCKET").map_err(|_| "MEMORY_BUCKET environment variable not set")?;

    let subscriptions_table = std::env::var("SUBSCRIPTIONS_TABLE")
        .map_err(|_| "SUBSCRIPTIONS_TABLE environment variable not set")?;

    let memories_table = std::env::var("MEMORIES_TABLE")
        .map_err(|_| "MEMORIES_TABLE environment variable not set")?;

    // Check subscription tier (Pro/Enterprise only)
    match check_subscription_tier(&dynamodb_client, &subscriptions_table, &user_id).await {
        Ok(tier) => {
            if !is_premium_tier(&tier) {
                return Ok(Response::builder()
                    .status(403)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&json!({
                        "error": "premium_required",
                        "message": "Memory cards feature requires Pro or Enterprise subscription",
                        "current_tier": tier
                    }))?))
                    .map_err(Box::new)?);
            }
        }
        Err(e) => {
            error!("Failed to check subscription tier: {}", e);
            return Ok(Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "internal_error",
                    "message": "Failed to validate subscription"
                }))?))
                .map_err(Box::new)?);
        }
    }

    // Verify memory ownership
    match verify_memory_ownership(&dynamodb_client, &memories_table, memory_id, &user_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Ok(Response::builder()
                .status(404)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "memory_not_found",
                    "message": "Memory not found or access denied"
                }))?))
                .map_err(Box::new)?);
        }
        Err(e) => {
            error!("Failed to verify memory ownership: {}", e);
            return Ok(Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "internal_error",
                    "message": "Failed to verify memory access"
                }))?))
                .map_err(Box::new)?);
        }
    }

    // Check if memory is enriched
    match check_enrichment_status(&dynamodb_client, &memories_table, memory_id).await {
        Ok(status) => {
            if status != "complete" {
                return Ok(Response::builder()
                    .status(409)
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&json!({
                        "error": "not_enriched",
                        "message": "Memory has not been enriched yet",
                        "enrichment_status": status
                    }))?))
                    .map_err(Box::new)?);
            }
        }
        Err(e) => {
            warn!("Failed to check enrichment status: {}", e);
            // Continue anyway - maybe it's an older memory without status field
        }
    }

    // Extract memory cards using memvid
    match extract_memory_cards(&s3_client, &bucket_name, memory_id).await {
        Ok(cards) => {
            info!(
                "Successfully extracted {} memory cards for memory {}",
                cards.len(),
                memory_id
            );
            Ok(Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "memory_id": memory_id,
                    "cards": cards,
                    "count": cards.len()
                }))?))
                .map_err(Box::new)?)
        }
        Err(e) => {
            error!("Failed to extract memory cards: {}", e);
            Ok(Response::builder()
                .status(500)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&json!({
                    "error": "extraction_failed",
                    "message": "Failed to extract memory cards"
                }))?))
                .map_err(Box::new)?)
        }
    }
}

async fn check_subscription_tier(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    table_name: &str,
    user_id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let result = dynamodb_client
        .get_item()
        .table_name(table_name)
        .key("userId", AttributeValue::S(user_id.to_string()))
        .send()
        .await?;

    if let Some(item) = result.item() {
        if let Some(tier_attr) = item.get("tier") {
            if let Ok(tier) = tier_attr.as_s() {
                return Ok(tier.clone());
            }
        }
    }

    Ok("free".to_string()) // Default to free if no subscription found
}

fn is_premium_tier(tier: &str) -> bool {
    matches!(tier, "pro" | "enterprise" | "premium")
}

async fn verify_memory_ownership(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    table_name: &str,
    memory_id: &str,
    user_id: &str,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let result = dynamodb_client
        .get_item()
        .table_name(table_name)
        .key("memoryId", AttributeValue::S(memory_id.to_string()))
        .send()
        .await?;

    if let Some(item) = result.item() {
        if let Some(owner_attr) = item.get("userId") {
            if let Ok(owner_id) = owner_attr.as_s() {
                return Ok(owner_id == user_id);
            }
        }
    }

    Ok(false)
}

async fn check_enrichment_status(
    dynamodb_client: &aws_sdk_dynamodb::Client,
    table_name: &str,
    memory_id: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let result = dynamodb_client
        .get_item()
        .table_name(table_name)
        .key("memoryId", AttributeValue::S(memory_id.to_string()))
        .send()
        .await?;

    if let Some(item) = result.item() {
        if let Some(status_attr) = item.get("enrichmentStatus") {
            if let Ok(status) = status_attr.as_s() {
                return Ok(status.clone());
            }
        }
    }

    Ok("unknown".to_string())
}

async fn extract_memory_cards(
    s3_client: &aws_sdk_s3::Client,
    bucket_name: &str,
    memory_id: &str,
) -> Result<Vec<Value>, Box<dyn std::error::Error + Send + Sync>> {
    let s3_key = format!("memories/{}.mv2", memory_id);

    // Download .mv2 file
    let obj = s3_client
        .get_object()
        .bucket(bucket_name)
        .key(&s3_key)
        .send()
        .await?;

    let data = obj.body.collect().await?.into_bytes();

    // Save to temporary file
    let mut temp_file = NamedTempFile::new()?;
    temp_file.write_all(&data)?;
    temp_file.flush()?;

    // Run memvid memories command
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = Command::new(memvid_path)
        .arg("memories")
        .arg("--json")
        .arg(temp_file.path())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("memvid memories failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON output - could be JSONL or a single JSON array
    let mut cards = Vec::new();

    // Try parsing as single JSON array first
    if let Ok(json_array) = serde_json::from_str::<Vec<Value>>(&stdout) {
        cards = json_array;
    } else {
        // Try parsing as JSONL (one JSON object per line)
        for line in stdout.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(json_obj) = serde_json::from_str::<Value>(line) {
                cards.push(json_obj);
            }
        }
    }

    Ok(cards)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}
