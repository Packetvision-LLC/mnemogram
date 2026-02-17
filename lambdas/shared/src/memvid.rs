use crate::errors::MnemogramError;
use aws_sdk_s3::Client as S3Client;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use tempfile::NamedTempFile;
use tokio::process::Command as AsyncCommand;

/// Result from memvid find command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemvidSearchResult {
    pub snippet: String,
    pub score: f64,
    pub timestamp: Option<String>,
    pub frame_id: Option<String>,
    pub uri: Option<String>,
}

/// Result from memvid ask command  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemvidAskResult {
    pub answer: String,
    pub sources: Vec<MemvidSearchResult>,
}

/// MemVid integration client
pub struct MemvidClient {
    s3_client: S3Client,
    bucket: String,
}

impl MemvidClient {
    pub fn new(s3_client: S3Client, bucket: String) -> Self {
        Self { s3_client, bucket }
    }

    /// Download .mv2 file from S3 to a temporary location
    async fn download_mv2_file(&self, memory_id: &str) -> Result<NamedTempFile, MnemogramError> {
        let key = format!("memories/{}.mv2", memory_id);

        let obj = self
            .s3_client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| MnemogramError::S3Error(format!("Failed to download .mv2 file: {}", e)))?;

        let data: bytes::Bytes = obj
            .body
            .collect()
            .await
            .map_err(|e| MnemogramError::S3Error(format!("Failed to read .mv2 file data: {}", e)))?
            .into_bytes();

        let mut temp_file = NamedTempFile::new()
            .map_err(|e| MnemogramError::Internal(format!("Failed to create temp file: {}", e)))?;

        temp_file
            .write_all(&data)
            .map_err(|e| MnemogramError::Internal(format!("Failed to write temp file: {}", e)))?;

        temp_file
            .flush()
            .map_err(|e| MnemogramError::Internal(format!("Failed to flush temp file: {}", e)))?;

        Ok(temp_file)
    }

    /// Perform lexical/semantic search using `memvid find`
    pub async fn search(
        &self,
        memory_id: &str,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<MemvidSearchResult>, MnemogramError> {
        let temp_file: NamedTempFile = self.download_mv2_file(memory_id).await?;

        // Use Lambda layer path if available, fallback to local path
        let memvid_path = if Path::new("/opt/bin/memvid").exists() {
            "/opt/bin/memvid"
        } else {
            "/home/stuart/.npm-global/bin/memvid"
        };

        let output = AsyncCommand::new(memvid_path)
            .arg("find")
            .arg("--query")
            .arg(query)
            .arg("--json")
            .arg("--top-k")
            .arg(top_k.to_string())
            .arg("--mode")
            .arg("auto") // auto chooses between lexical and semantic based on query
            .arg(temp_file.path())
            .output()
            .await
            .map_err(|e| {
                MnemogramError::ExternalService(format!("Failed to execute memvid find: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MnemogramError::ExternalService(format!(
                "memvid find failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON output from memvid find
        self.parse_find_output(&stdout)
    }

    /// Ask questions using `memvid ask` with retrieval + synthesis
    pub async fn ask(
        &self,
        memory_id: &str,
        question: &str,
        top_k: usize,
    ) -> Result<MemvidAskResult, MnemogramError> {
        let temp_file: NamedTempFile = self.download_mv2_file(memory_id).await?;

        // Use Lambda layer path if available, fallback to local path
        let memvid_path = if Path::new("/opt/bin/memvid").exists() {
            "/opt/bin/memvid"
        } else {
            "/home/stuart/.npm-global/bin/memvid"
        };

        let output = AsyncCommand::new(memvid_path)
            .arg("ask")
            .arg("--question")
            .arg(question)
            .arg("--json")
            .arg("--top-k")
            .arg(top_k.to_string())
            .arg("--sources") // Include source information
            .arg("--no-llm") // Return just evidence without LLM synthesis for now
            .arg(temp_file.path())
            .output()
            .await
            .map_err(|e| {
                MnemogramError::ExternalService(format!("Failed to execute memvid ask: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MnemogramError::ExternalService(format!(
                "memvid ask failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse JSON output from memvid ask
        self.parse_ask_output(&stdout)
    }

    /// Parse output from memvid find command
    fn parse_find_output(&self, output: &str) -> Result<Vec<MemvidSearchResult>, MnemogramError> {
        // memvid find --json returns JSONL format (one JSON object per line)
        let mut results = Vec::new();

        for line in output.lines() {
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as JSON - memvid CLI output format may vary
            if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(line) {
                let result = MemvidSearchResult {
                    snippet: json_value
                        .get("text")
                        .or_else(|| json_value.get("snippet"))
                        .or_else(|| json_value.get("content"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("No content")
                        .to_string(),
                    score: json_value
                        .get("score")
                        .or_else(|| json_value.get("relevancy"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0),
                    timestamp: json_value
                        .get("timestamp")
                        .or_else(|| json_value.get("time"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    frame_id: json_value
                        .get("frame_id")
                        .or_else(|| json_value.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    uri: json_value
                        .get("uri")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                };

                results.push(result);
            }
        }

        Ok(results)
    }

    /// Parse output from memvid ask command
    fn parse_ask_output(&self, output: &str) -> Result<MemvidAskResult, MnemogramError> {
        // For now, treat ask output similarly to find since we're using --no-llm
        let sources = self.parse_find_output(output)?;

        // Create a simple concatenated answer from top results
        let answer = if sources.is_empty() {
            "No relevant information found.".to_string()
        } else {
            sources
                .iter()
                .take(3) // Top 3 snippets
                .map(|r| r.snippet.clone())
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        Ok(MemvidAskResult { answer, sources })
    }

    /// Validate .mv2 file format by checking magic bytes
    pub async fn validate_mv2_file(&self, memory_id: &str) -> Result<bool, MnemogramError> {
        let key = format!("memories/{}.mv2", memory_id);

        // Read just the first few bytes to check magic bytes
        let obj = self
            .s3_client
            .get_object()
            .bucket(&self.bucket)
            .key(&key)
            .range("bytes=0-15") // Read first 16 bytes
            .send()
            .await
            .map_err(|e| {
                MnemogramError::S3Error(format!("Failed to read .mv2 file header: {}", e))
            })?;

        let data: bytes::Bytes = obj
            .body
            .collect()
            .await
            .map_err(|e| {
                MnemogramError::S3Error(format!("Failed to read .mv2 file header data: {}", e))
            })?
            .into_bytes();

        // Check if this looks like a valid .mv2 file
        // The exact magic bytes depend on the memvid format - we'll check for common signatures
        let bytes = &data;

        // Common file signatures that might be used by .mv2 files:
        // - Could be MP4-based: starts with "ftyp" at offset 4
        // - Could be custom binary format
        // - For now, just check it's not empty and has some structure

        if bytes.len() < 8 {
            return Ok(false);
        }

        // Simple heuristic - if it looks like binary data with reasonable structure
        let has_structure = bytes.iter().any(|&b| b == 0) && // Contains null bytes (binary)
                           bytes.iter().any(|&b| b > 32); // Contains printable chars too

        Ok(has_structure)
    }
}

/// Check if memvid CLI is available
pub fn is_memvid_cli_available() -> bool {
    Path::new("/opt/bin/memvid").exists()
        || Path::new("/home/stuart/.npm-global/bin/memvid").exists()
}

/// Get memvid CLI version
pub fn get_memvid_version() -> Result<String, MnemogramError> {
    let memvid_path = if Path::new("/opt/bin/memvid").exists() {
        "/opt/bin/memvid"
    } else {
        "/home/stuart/.npm-global/bin/memvid"
    };

    let output = Command::new(memvid_path)
        .arg("--version")
        .output()
        .map_err(|e| {
            MnemogramError::ExternalService(format!("Failed to get memvid version: {}", e))
        })?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(MnemogramError::ExternalService(
            "Failed to get memvid version".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memvid_cli_available() {
        // This test will pass in the Lambda environment where memvid is bundled
        println!("Memvid CLI available: {}", is_memvid_cli_available());
    }
}
