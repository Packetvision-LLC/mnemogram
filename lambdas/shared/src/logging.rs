use serde_json::{json, Value};
use std::collections::HashMap;
use tracing::{event, Level};

/// Structured logging helper for CloudWatch
pub struct StructuredLogger;

impl StructuredLogger {
    /// Log an info event with structured data
    pub fn info(message: &str, fields: HashMap<&str, Value>) {
        let mut log_data = json!({
            "level": "INFO",
            "message": message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        if let Value::Object(ref mut map) = log_data {
            for (key, value) in fields {
                map.insert(key.to_string(), value);
            }
        }

        event!(Level::INFO, "{}", log_data);
    }

    /// Log an error event with structured data
    pub fn error(message: &str, fields: HashMap<&str, Value>) {
        let mut log_data = json!({
            "level": "ERROR",
            "message": message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        if let Value::Object(ref mut map) = log_data {
            for (key, value) in fields {
                map.insert(key.to_string(), value);
            }
        }

        event!(Level::ERROR, "{}", log_data);
    }

    /// Log a warning event with structured data  
    pub fn warn(message: &str, fields: HashMap<&str, Value>) {
        let mut log_data = json!({
            "level": "WARN",
            "message": message,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        if let Value::Object(ref mut map) = log_data {
            for (key, value) in fields {
                map.insert(key.to_string(), value);
            }
        }

        event!(Level::WARN, "{}", log_data);
    }

    /// Log request/response for API calls
    pub fn api_call(
        request_id: &str,
        method: &str,
        path: &str,
        status_code: u16,
        duration_ms: u64,
        user_id: Option<&str>,
    ) {
        let mut fields = HashMap::new();
        fields.insert("requestId", json!(request_id));
        fields.insert("method", json!(method));
        fields.insert("path", json!(path));
        fields.insert("statusCode", json!(status_code));
        fields.insert("durationMs", json!(duration_ms));
        
        if let Some(uid) = user_id {
            fields.insert("userId", json!(uid));
        }

        Self::info("API call completed", fields);
    }

    /// Log database operations
    pub fn database_operation(
        request_id: &str,
        operation: &str,
        table: &str,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) {
        let mut fields = HashMap::new();
        fields.insert("requestId", json!(request_id));
        fields.insert("operation", json!(operation));
        fields.insert("table", json!(table));
        fields.insert("durationMs", json!(duration_ms));
        fields.insert("success", json!(success));
        
        if let Some(err) = error {
            fields.insert("error", json!(err));
        }

        if success {
            Self::info("Database operation completed", fields);
        } else {
            Self::error("Database operation failed", fields);
        }
    }

    /// Log external service calls
    pub fn external_service_call(
        request_id: &str,
        service: &str,
        operation: &str,
        duration_ms: u64,
        status_code: Option<u16>,
        success: bool,
        error: Option<&str>,
    ) {
        let mut fields = HashMap::new();
        fields.insert("requestId", json!(request_id));
        fields.insert("service", json!(service));
        fields.insert("operation", json!(operation));
        fields.insert("durationMs", json!(duration_ms));
        fields.insert("success", json!(success));
        
        if let Some(code) = status_code {
            fields.insert("statusCode", json!(code));
        }
        
        if let Some(err) = error {
            fields.insert("error", json!(err));
        }

        if success {
            Self::info("External service call completed", fields);
        } else {
            Self::error("External service call failed", fields);
        }
    }

    /// Log memory/processing operations
    pub fn memory_operation(
        request_id: &str,
        operation: &str,
        memory_id: Option<&str>,
        file_size_bytes: Option<u64>,
        duration_ms: u64,
        success: bool,
        error: Option<&str>,
    ) {
        let mut fields = HashMap::new();
        fields.insert("requestId", json!(request_id));
        fields.insert("operation", json!(operation));
        fields.insert("durationMs", json!(duration_ms));
        fields.insert("success", json!(success));
        
        if let Some(mid) = memory_id {
            fields.insert("memoryId", json!(mid));
        }
        
        if let Some(size) = file_size_bytes {
            fields.insert("fileSizeBytes", json!(size));
        }
        
        if let Some(err) = error {
            fields.insert("error", json!(err));
        }

        if success {
            Self::info("Memory operation completed", fields);
        } else {
            Self::error("Memory operation failed", fields);
        }
    }

    /// Log authentication events
    pub fn auth_event(
        request_id: &str,
        event_type: &str, // "login", "logout", "token_refresh", etc.
        user_id: Option<&str>,
        success: bool,
        error: Option<&str>,
    ) {
        let mut fields = HashMap::new();
        fields.insert("requestId", json!(request_id));
        fields.insert("eventType", json!(event_type));
        fields.insert("success", json!(success));
        
        if let Some(uid) = user_id {
            fields.insert("userId", json!(uid));
        }
        
        if let Some(err) = error {
            fields.insert("error", json!(err));
        }

        if success {
            Self::info("Authentication event", fields);
        } else {
            Self::warn("Authentication event failed", fields);
        }
    }
}

/// Initialize logging for Lambda functions
pub fn init_logging() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_file(false)
        .with_line_number(false)
        .json()
        .init();
}

/// Create log context fields for a request
pub fn create_log_context(request_id: &str, function_name: &str) -> HashMap<&'static str, Value> {
    let mut fields = HashMap::new();
    fields.insert("requestId", json!(request_id));
    fields.insert("functionName", json!(function_name));
    fields
}