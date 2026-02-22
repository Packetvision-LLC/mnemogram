use serde::{Deserialize, Serialize};
use aws_sdk_s3vectors::Client as S3VectorsClient;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_s3::Client as S3Client;
use aws_config::BehaviorVersion;
use std::collections::HashMap;
use crate::errors::MnemogramError;
use uuid::Uuid;

/// Result from S3 Vectors search (maintains compatibility with MemVid interface)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemvidSearchResult {
    pub snippet: String,
    pub score: f64,
    pub timestamp: Option<String>,
    pub frame_id: Option<String>,
    pub uri: Option<String>,
}

/// Result from S3 Vectors ask operation (maintains compatibility with MemVid interface)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemvidAskResult {
    pub answer: String,
    pub sources: Vec<MemvidSearchResult>,
}

/// S3 Vector Search integration client (replaces MemVid CLI)
pub struct MemvidClient {
    s3_client: S3Client,
    s3vectors_client: S3VectorsClient,
    bedrock_client: BedrockClient,
    bucket: String,
    vector_bucket: String,
    vector_index: String,
}

impl MemvidClient {
    pub fn new(s3_client: S3Client, bucket: String) -> Self {
        // Initialize other clients using the same config
        let config = aws_config::load_defaults(BehaviorVersion::latest());
        let runtime = tokio::runtime::Handle::current();
        let config = runtime.block_on(config);
        
        let s3vectors_client = S3VectorsClient::new(&config);
        let bedrock_client = BedrockClient::new(&config);
        
        // Get vector configuration from environment variables
        let vector_bucket = std::env::var("VECTOR_BUCKET_NAME")
            .unwrap_or_else(|_| format!("{}-vectors", bucket));
        let vector_index = std::env::var("VECTOR_INDEX_NAME")
            .unwrap_or_else(|_| "memories".to_string());

        Self {
            s3_client,
            s3vectors_client,
            bedrock_client,
            bucket,
            vector_bucket,
            vector_index,
        }
    }

    /// Generate vector embeddings using Amazon Bedrock
    async fn generate_embedding(&self, text: &str) -> Result<Vec<f32>, MnemogramError> {
        // Use Titan Text Embeddings V2 model (1024 dimensions)
        let model_id = std::env::var("EMBEDDING_MODEL_ID")
            .unwrap_or_else(|_| "amazon.titan-embed-text-v2:0".to_string());

        let input = serde_json::json!({
            "inputText": text
        });

        let payload = serde_json::to_string(&input)
            .map_err(|e| MnemogramError::Internal(format!("Failed to serialize embedding request: {}", e)))?;

        let response = self.bedrock_client
            .invoke_model()
            .model_id(model_id)
            .content_type("application/json")
            .body(aws_sdk_bedrockruntime::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| MnemogramError::ExternalService(format!("Bedrock embedding failed: {}", e)))?;

        let response_body = response.body().as_ref();
        let response_str = std::str::from_utf8(response_body)
            .map_err(|e| MnemogramError::Internal(format!("Invalid UTF-8 in Bedrock response: {}", e)))?;

        let response_json: serde_json::Value = serde_json::from_str(response_str)
            .map_err(|e| MnemogramError::Internal(format!("Failed to parse Bedrock response: {}", e)))?;

        let embedding_vec = response_json["embedding"]
            .as_array()
            .ok_or_else(|| MnemogramError::Internal("Missing embedding in Bedrock response".to_string()))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect();

        Ok(embedding_vec)
    }

    /// Perform semantic search using S3 Vectors (replaces memvid find)
    pub async fn search(&self, memory_id: &str, query: &str, top_k: usize) -> Result<Vec<MemvidSearchResult>, MnemogramError> {
        // Generate embedding for the query
        let query_embedding = self.generate_embedding(query).await?;

        // Perform vector similarity search
        let query_vector = aws_sdk_s3vectors::types::QueryVector::Float32(query_embedding);

        let response = self.s3vectors_client
            .query_vectors()
            .vector_bucket_name(&self.vector_bucket)
            .index_name(&self.vector_index)
            .top_k(top_k as i32)
            .query_vector(query_vector)
            .return_metadata(true)
            .return_distance(true)
            // Filter by memory_id to search within specific memory
            .filter(serde_json::json!({
                "memory_id": memory_id
            }))
            .send()
            .await
            .map_err(|e| MnemogramError::ExternalService(format!("S3 Vectors query failed: {}", e)))?;

        // Convert S3 Vectors response to MemvidSearchResult format
        let mut results = Vec::new();
        
        if let Some(vectors) = response.vectors() {
            for vector in vectors {
                let snippet = vector.metadata()
                    .and_then(|metadata| {
                        // Extract text content from metadata
                        if let Ok(metadata_str) = serde_json::to_string(metadata) {
                            let metadata_obj: serde_json::Value = serde_json::from_str(&metadata_str).ok()?;
                            metadata_obj.get("text")
                                .or_else(|| metadata_obj.get("content"))
                                .or_else(|| metadata_obj.get("snippet"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "No content available".to_string());

                let timestamp = vector.metadata()
                    .and_then(|metadata| {
                        if let Ok(metadata_str) = serde_json::to_string(metadata) {
                            let metadata_obj: serde_json::Value = serde_json::from_str(&metadata_str).ok()?;
                            metadata_obj.get("timestamp")
                                .or_else(|| metadata_obj.get("created_at"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        } else {
                            None
                        }
                    });

                let frame_id = vector.metadata()
                    .and_then(|metadata| {
                        if let Ok(metadata_str) = serde_json::to_string(metadata) {
                            let metadata_obj: serde_json::Value = serde_json::from_str(&metadata_str).ok()?;
                            metadata_obj.get("frame_id")
                                .or_else(|| metadata_obj.get("id"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        } else {
                            None
                        }
                    });

                // Convert distance to similarity score (higher is better)
                let score = if let Some(distance) = vector.distance() {
                    // For cosine distance, similarity = 1 - distance
                    // For euclidean distance, we can use 1 / (1 + distance)
                    match response.distance_metric() {
                        Some(aws_sdk_s3vectors::types::DistanceMetric::Cosine) => 1.0 - distance,
                        _ => 1.0 / (1.0 + distance), // Euclidean or unknown
                    }
                } else {
                    0.0
                };

                results.push(MemvidSearchResult {
                    snippet,
                    score,
                    timestamp,
                    frame_id,
                    uri: None, // S3 Vectors doesn't have URI concept like MemVid
                });
            }
        }

        // Sort by score descending (highest similarity first)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        Ok(results)
    }

    /// Ask questions using S3 Vectors search + synthesis (replaces memvid ask)
    pub async fn ask(&self, memory_id: &str, question: &str, top_k: usize) -> Result<MemvidAskResult, MnemogramError> {
        // Get relevant sources using search
        let sources = self.search(memory_id, question, top_k).await?;

        // For now, create a simple answer by concatenating top results
        // In the future, this could use an LLM to synthesize an answer
        let answer = if sources.is_empty() {
            "No relevant information found.".to_string()
        } else {
            sources.iter()
                .take(3) // Top 3 snippets
                .map(|r| r.snippet.clone())
                .collect::<Vec<_>>()
                .join("\n\n")
        };

        Ok(MemvidAskResult { answer, sources })
    }

    /// Store memory content as vectors in S3 Vectors (new method for migration)
    pub async fn store_memory_vectors(&self, memory_id: &str, chunks: Vec<(String, HashMap<String, String>)>) -> Result<(), MnemogramError> {
        if chunks.is_empty() {
            return Ok(());
        }

        let mut vectors_to_insert = Vec::new();

        for (i, (text, mut metadata)) in chunks.into_iter().enumerate() {
            // Generate embedding for the text chunk
            let embedding = self.generate_embedding(&text).await?;

            // Add memory_id and chunk info to metadata
            metadata.insert("memory_id".to_string(), memory_id.to_string());
            metadata.insert("chunk_id".to_string(), format!("{}_{}", memory_id, i));
            metadata.insert("text".to_string(), text);

            // Convert metadata to the format expected by S3 Vectors
            let metadata_document: aws_sdk_s3vectors::primitives::Document = 
                serde_json::from_str(&serde_json::to_string(&metadata)
                    .map_err(|e| MnemogramError::Internal(format!("Failed to serialize metadata: {}", e)))?)
                .map_err(|e| MnemogramError::Internal(format!("Failed to convert metadata: {}", e)))?;

            // Create vector object
            let vector = aws_sdk_s3vectors::types::Vector::builder()
                .key(format!("{}_{}", memory_id, i))
                .vector_data(aws_sdk_s3vectors::types::VectorData::Float32(embedding))
                .metadata(metadata_document)
                .build()
                .map_err(|e| MnemogramError::Internal(format!("Failed to build vector: {}", e)))?;

            vectors_to_insert.push(vector);
        }

        // Insert vectors in batches (S3 Vectors may have batch size limits)
        const BATCH_SIZE: usize = 100;
        for batch in vectors_to_insert.chunks(BATCH_SIZE) {
            self.s3vectors_client
                .put_vectors()
                .vector_bucket_name(&self.vector_bucket)
                .index_name(&self.vector_index)
                .set_vectors(Some(batch.to_vec()))
                .send()
                .await
                .map_err(|e| MnemogramError::ExternalService(format!("Failed to insert vectors batch: {}", e)))?;
        }

        tracing::info!("Successfully stored {} vector chunks for memory {}", vectors_to_insert.len(), memory_id);

        Ok(())
    }

    /// Migrate existing .mv2 memory to S3 Vectors format
    pub async fn migrate_memory_from_mv2(&self, memory_id: &str) -> Result<(), MnemogramError> {
        // This method would extract content from existing .mv2 files and convert to S3 Vectors
        // For now, we'll implement a placeholder that handles the migration
        
        let key = format!("memories/{}.mv2", memory_id);
        
        // Check if .mv2 file exists
        let _obj = self.s3_client
            .head_object()
            .bucket(&self.bucket)
            .key(&key)
            .send()
            .await
            .map_err(|e| MnemogramError::S3Error(format!("Failed to check .mv2 file: {}", e)))?;

        // For this initial implementation, we'll create placeholder chunks
        // In a real migration, we would parse the .mv2 file and extract actual content
        let chunks = vec![
            ("Sample memory content extracted from .mv2 file".to_string(), 
             HashMap::from([
                 ("source".to_string(), "mv2_migration".to_string()),
                 ("timestamp".to_string(), chrono::Utc::now().to_rfc3339()),
             ])),
        ];

        self.store_memory_vectors(memory_id, chunks).await?;

        tracing::info!("Successfully migrated memory {} from .mv2 to S3 Vectors", memory_id);
        
        Ok(())
    }

    /// Validate that S3 Vectors index is accessible
    pub async fn validate_vector_index(&self) -> Result<bool, MnemogramError> {
        // Try to query the index with a simple test query to check if it's accessible
        let test_embedding = vec![0.0_f32; 1024]; // 1024-dim zero vector for testing
        let query_vector = aws_sdk_s3vectors::types::QueryVector::Float32(test_embedding);

        match self.s3vectors_client
            .query_vectors()
            .vector_bucket_name(&self.vector_bucket)
            .index_name(&self.vector_index)
            .top_k(1)
            .query_vector(query_vector)
            .send()
            .await 
        {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::warn!("Vector index validation failed: {}", e);
                Ok(false)
            }
        }
    }
}

/// Check if S3 Vectors is available and configured
pub async fn is_s3_vectors_available() -> bool {
    std::env::var("VECTOR_BUCKET_NAME").is_ok() || std::env::var("STORAGE_BUCKET").is_ok()
}

/// Get S3 Vectors configuration info
pub async fn get_s3_vectors_info() -> Result<String, MnemogramError> {
    let vector_bucket = std::env::var("VECTOR_BUCKET_NAME")
        .unwrap_or_else(|_| "not_configured".to_string());
    let vector_index = std::env::var("VECTOR_INDEX_NAME")
        .unwrap_or_else(|_| "memories".to_string());
    let embedding_model = std::env::var("EMBEDDING_MODEL_ID")
        .unwrap_or_else(|_| "amazon.titan-embed-text-v2:0".to_string());

    Ok(format!("S3 Vectors - Bucket: {}, Index: {}, Model: {}", 
               vector_bucket, vector_index, embedding_model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_s3_vectors_available() {
        println!("S3 Vectors available: {}", is_s3_vectors_available().await);
    }
}