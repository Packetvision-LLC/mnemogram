use aws_sdk_dynamodb::{types::AttributeValue, Client as DynamoDbClient};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, info, warn};

/// Usage event types for tracking different API operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UsageEventType {
    IngestMemory,
    SearchMemory,
    RecallMemory,
    ManageMemory,
    BatchRecall,
    FactExtraction,
    Enrichment,
}

impl UsageEventType {
    pub fn as_str(&self) -> &'static str {
        match self {
            UsageEventType::IngestMemory => "ingest_memory",
            UsageEventType::SearchMemory => "search_memory",
            UsageEventType::RecallMemory => "recall_memory",
            UsageEventType::ManageMemory => "manage_memory",
            UsageEventType::BatchRecall => "batch_recall",
            UsageEventType::FactExtraction => "fact_extraction",
            UsageEventType::Enrichment => "enrichment",
        }
    }
}

/// Usage tracking event with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub user_id: String,
    pub event_type: UsageEventType,
    pub timestamp: DateTime<Utc>,
    pub request_id: String,
    pub metadata: Option<HashMap<String, String>>,
}

/// Usage tracker for recording API usage in DynamoDB
pub struct UsageTracker {
    dynamodb_client: DynamoDbClient,
    usage_table_name: String,
}

impl UsageTracker {
    /// Create a new usage tracker instance
    pub fn new(dynamodb_client: DynamoDbClient, usage_table_name: String) -> Self {
        Self {
            dynamodb_client,
            usage_table_name,
        }
    }

    /// Track a usage event asynchronously
    pub async fn track_usage(&self, event: UsageEvent) -> Result<(), crate::errors::MnemogramError> {
        let date = event.timestamp.format("%Y-%m-%d").to_string();
        let timestamp_key = event.timestamp.format("%Y-%m-%d-%H:%M:%S%.3f").to_string();
        
        let mut item = HashMap::new();
        item.insert(
            "userId".to_string(),
            AttributeValue::S(event.user_id.clone()),
        );
        item.insert(
            "date".to_string(),
            AttributeValue::S(date),
        );
        item.insert(
            "timestamp".to_string(),
            AttributeValue::S(timestamp_key),
        );
        item.insert(
            "eventType".to_string(),
            AttributeValue::S(event.event_type.as_str().to_string()),
        );
        item.insert(
            "requestId".to_string(),
            AttributeValue::S(event.request_id),
        );

        // Add metadata if present
        if let Some(metadata) = &event.metadata {
            let metadata_map: HashMap<String, AttributeValue> = metadata
                .iter()
                .map(|(k, v)| (k.clone(), AttributeValue::S(v.clone())))
                .collect();
            
            item.insert(
                "metadata".to_string(),
                AttributeValue::M(metadata_map),
            );
        }

        match self
            .dynamodb_client
            .put_item()
            .table_name(&self.usage_table_name)
            .set_item(Some(item))
            .send()
            .await
        {
            Ok(_) => {
                info!(
                    user_id = %event.user_id,
                    event_type = %event.event_type.as_str(),
                    table = %self.usage_table_name,
                    "Usage event tracked successfully"
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    user_id = %event.user_id,
                    event_type = %event.event_type.as_str(),
                    error = %e,
                    "Failed to track usage event"
                );
                // Don't fail the request if usage tracking fails, just log the error
                warn!("Usage tracking failed but continuing with request");
                Ok(())
            }
        }
    }

    /// Get daily usage count for a user and event type
    pub async fn get_daily_usage_count(
        &self,
        user_id: &str,
        event_type: &UsageEventType,
        date: &str,
    ) -> Result<i32, crate::errors::MnemogramError> {
        let response = self
            .dynamodb_client
            .query()
            .table_name(&self.usage_table_name)
            .key_condition_expression("userId = :user_id AND #date = :date")
            .filter_expression("eventType = :event_type")
            .expression_attribute_names("#date", "date")
            .expression_attribute_values(":user_id", AttributeValue::S(user_id.to_string()))
            .expression_attribute_values(":date", AttributeValue::S(date.to_string()))
            .expression_attribute_values(":event_type", AttributeValue::S(event_type.as_str().to_string()))
            .send()
            .await
            .map_err(|e| crate::errors::MnemogramError::DatabaseError(e.to_string()))?;

        Ok(response.items.unwrap_or_default().len() as i32)
    }

    /// Get monthly usage summary for a user
    pub async fn get_monthly_usage_summary(
        &self,
        user_id: &str,
        year_month: &str, // Format: YYYY-MM
    ) -> Result<HashMap<String, i32>, crate::errors::MnemogramError> {
        let response = self
            .dynamodb_client
            .query()
            .table_name(&self.usage_table_name)
            .key_condition_expression("userId = :user_id AND begins_with(#date, :year_month)")
            .expression_attribute_names("#date", "date")
            .expression_attribute_values(":user_id", AttributeValue::S(user_id.to_string()))
            .expression_attribute_values(":year_month", AttributeValue::S(year_month.to_string()))
            .send()
            .await
            .map_err(|e| crate::errors::MnemogramError::DatabaseError(e.to_string()))?;

        let mut summary = HashMap::new();
        
        for item in response.items.unwrap_or_default() {
            if let Some(AttributeValue::S(event_type)) = item.get("eventType") {
                *summary.entry(event_type.clone()).or_insert(0) += 1;
            }
        }

        Ok(summary)
    }
}

/// Middleware function to track usage before processing requests
pub async fn track_api_usage(
    user_id: String,
    event_type: UsageEventType,
    request_id: String,
    metadata: Option<HashMap<String, String>>,
) -> Result<(), crate::errors::MnemogramError> {
    // Get table name from environment
    let usage_table_name = std::env::var("USAGE_TABLE_NAME")
        .unwrap_or_else(|_| "mnemogram-dev-usage".to_string());

    // Initialize AWS config and DynamoDB client
    let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);
    
    let tracker = UsageTracker::new(dynamodb_client, usage_table_name);
    
    let event = UsageEvent {
        user_id,
        event_type,
        timestamp: Utc::now(),
        request_id,
        metadata,
    };

    tracker.track_usage(event).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_usage_event_type_as_str() {
        assert_eq!(UsageEventType::IngestMemory.as_str(), "ingest_memory");
        assert_eq!(UsageEventType::SearchMemory.as_str(), "search_memory");
        assert_eq!(UsageEventType::RecallMemory.as_str(), "recall_memory");
    }

    #[test]
    fn test_usage_event_creation() {
        let mut metadata = HashMap::new();
        metadata.insert("file_size".to_string(), "1024".to_string());
        
        let event = UsageEvent {
            user_id: "test-user-123".to_string(),
            event_type: UsageEventType::IngestMemory,
            timestamp: Utc::now(),
            request_id: "req-123".to_string(),
            metadata: Some(metadata),
        };

        assert_eq!(event.user_id, "test-user-123");
        assert_eq!(event.event_type.as_str(), "ingest_memory");
        assert!(event.metadata.is_some());
    }
}