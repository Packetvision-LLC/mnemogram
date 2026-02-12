use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use crate::errors::MnemogramError;
use tracing::{info, warn, error};

/// Cache TTL in seconds (5 minutes)
const CACHE_TTL_SECONDS: u64 = 300;

/// Get cache directory in Lambda /tmp
fn get_cache_dir() -> PathBuf {
    PathBuf::from("/tmp/mv2_cache")
}

/// Get cache file path for a given S3 key
fn get_cache_file_path(s3_key: &str) -> PathBuf {
    let mut cache_dir = get_cache_dir();
    // Replace path separators and special chars with underscores for safe filename
    let safe_filename = s3_key.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
    cache_dir.push(format!("{}.mv2", safe_filename));
    cache_dir
}

/// Check if cached file exists and is still valid (within TTL)
fn is_cache_valid(cache_path: &Path) -> bool {
    if !cache_path.exists() {
        return false;
    }

    match fs::metadata(cache_path) {
        Ok(metadata) => {
            if let Ok(modified) = metadata.modified() {
                if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                    let file_age_seconds = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs() - duration.as_secs();
                    
                    return file_age_seconds < CACHE_TTL_SECONDS;
                }
            }
            false
        },
        Err(_) => false,
    }
}

/// Download .mv2 file from S3 with caching
/// Returns the local file path to the cached .mv2 file
pub async fn get_cached_mv2_file(
    s3_client: &Client,
    bucket: &str,
    s3_key: &str,
) -> Result<PathBuf, MnemogramError> {
    let cache_path = get_cache_file_path(s3_key);
    
    // Check if we have a valid cached version
    if is_cache_valid(&cache_path) {
        info!("Using cached .mv2 file: {:?}", cache_path);
        return Ok(cache_path);
    }

    // Ensure cache directory exists
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| MnemogramError::Internal(format!("Failed to create cache directory: {}", e)))?;
    }

    info!("Downloading .mv2 file from S3: {}/{}", bucket, s3_key);

    // Download from S3
    let response = s3_client
        .get_object()
        .bucket(bucket)
        .key(s3_key)
        .send()
        .await
        .map_err(|e| MnemogramError::S3Error(format!("Failed to get object from S3: {}", e)))?;

    // Stream the body to a temporary file first, then rename atomically
    let temp_path = cache_path.with_extension("tmp");
    
    // Convert ByteStream to bytes
    let body_bytes = response
        .body
        .collect()
        .await
        .map_err(|e| MnemogramError::S3Error(format!("Failed to read S3 object body: {}", e)))?
        .into_bytes();

    // Write to temporary file
    fs::write(&temp_path, body_bytes)
        .map_err(|e| MnemogramError::Internal(format!("Failed to write cache file: {}", e)))?;

    // Atomic rename
    fs::rename(&temp_path, &cache_path)
        .map_err(|e| MnemogramError::Internal(format!("Failed to finalize cache file: {}", e)))?;

    info!("Cached .mv2 file at: {:?}", cache_path);
    Ok(cache_path)
}

/// Clean up old cache files (optional, can be called periodically)
pub fn cleanup_old_cache_files() -> Result<(), MnemogramError> {
    let cache_dir = get_cache_dir();
    if !cache_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(&cache_dir)
        .map_err(|e| MnemogramError::Internal(format!("Failed to read cache directory: {}", e)))?;

    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    for entry in entries {
        if let Ok(entry) = entry {
            if let Ok(metadata) = entry.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                        let file_age_seconds = current_time - duration.as_secs();
                        
                        // Remove files older than 2x TTL (10 minutes)
                        if file_age_seconds > CACHE_TTL_SECONDS * 2 {
                            if let Err(e) = fs::remove_file(entry.path()) {
                                warn!("Failed to remove old cache file {:?}: {}", entry.path(), e);
                            } else {
                                info!("Removed old cache file: {:?}", entry.path());
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Initialize S3 client
pub async fn init_s3_client() -> Client {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    Client::new(&config)
}