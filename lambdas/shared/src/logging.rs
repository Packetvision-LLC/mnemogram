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

        // Add X-Ray trace ID if available
        if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
            log_data["traceId"] = json!(trace_id);
        }

        // Add AWS request ID if available
        if let Ok(request_id) = std::env::var("AWS_REQUEST_ID") {
            log_data["awsRequestId"] = json!(request_id);
        }

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

        // Add X-Ray trace ID if available
        if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
            log_data["traceId"] = json!(trace_id);
        }

        // Add AWS request ID if available
        if let Ok(request_id) = std::env::var("AWS_REQUEST_ID") {
            log_data["awsRequestId"] = json!(request_id);
        }

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

        // Add X-Ray trace ID if available
        if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
            log_data["traceId"] = json!(trace_id);
        }

        // Add AWS request ID if available
        if let Ok(request_id) = std::env::var("AWS_REQUEST_ID") {
            log_data["awsRequestId"] = json!(request_id);
        }

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

/// Initialize logging for Lambda functions with X-Ray integration
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

/// Extract request ID from Lambda event context or generate one
pub fn get_or_generate_request_id(lambda_context: Option<&lambda_runtime::Context>) -> String {
    if let Some(ctx) = lambda_context {
        ctx.request_id.clone()
    } else {
        // Generate a new request ID if not available
        uuid::Uuid::new_v4().to_string()
    }
}

/// Create log context fields for a request with X-Ray trace integration
pub fn create_log_context(request_id: &str, function_name: &str) -> HashMap<&'static str, Value> {
    let mut fields = HashMap::new();
    fields.insert("requestId", json!(request_id));
    fields.insert("functionName", json!(function_name));

    // Add X-Ray trace ID if available
    if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
        fields.insert("traceId", json!(trace_id));
    }

    // Add AWS Lambda request ID if different from our request ID
    if let Ok(aws_request_id) = std::env::var("AWS_REQUEST_ID") {
        if aws_request_id != request_id {
            fields.insert("awsRequestId", json!(aws_request_id));
        }
    }

    fields
}

/// Macro to create a structured log entry with automatic request ID and trace context
#[macro_export]
macro_rules! log_with_context {
    ($level:expr, $message:expr, $request_id:expr) => {
        {
            let mut fields = std::collections::HashMap::new();
            fields.insert("requestId", serde_json::json!($request_id));

            // Add X-Ray trace ID if available
            if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
                fields.insert("traceId", serde_json::json!(trace_id));
            }

            match $level {
                "info" => $crate::logging::StructuredLogger::info($message, fields),
                "warn" => $crate::logging::StructuredLogger::warn($message, fields),
                "error" => $crate::logging::StructuredLogger::error($message, fields),
                _ => $crate::logging::StructuredLogger::info($message, fields),
            }
        }
    };

    ($level:expr, $message:expr, $request_id:expr, $($key:expr => $value:expr),+) => {
        {
            let mut fields = std::collections::HashMap::new();
            fields.insert("requestId", serde_json::json!($request_id));

            // Add X-Ray trace ID if available
            if let Ok(trace_id) = std::env::var("_X_AMZN_TRACE_ID") {
                fields.insert("traceId", serde_json::json!(trace_id));
            }

            $(
                fields.insert($key, serde_json::json!($value));
            )+

            match $level {
                "info" => $crate::logging::StructuredLogger::info($message, fields),
                "warn" => $crate::logging::StructuredLogger::warn($message, fields),
                "error" => $crate::logging::StructuredLogger::error($message, fields),
                _ => $crate::logging::StructuredLogger::info($message, fields),
            }
        }
    };
}
