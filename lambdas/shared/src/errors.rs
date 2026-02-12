use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MnemogramError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("validation failed: {0}")]
    ValidationError(String),

    #[error("rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("external service error: {0}")]
    ExternalService(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("s3 error: {0}")]
    S3Error(String),
}

impl MnemogramError {
    pub fn error_code(&self) -> &'static str {
        match self {
            MnemogramError::NotFound(_) => "NOT_FOUND",
            MnemogramError::Unauthorized(_) => "UNAUTHORIZED",
            MnemogramError::Forbidden(_) => "FORBIDDEN",
            MnemogramError::BadRequest(_) => "BAD_REQUEST",
            MnemogramError::ValidationError(_) => "VALIDATION_ERROR",
            MnemogramError::RateLimitExceeded(_) => "RATE_LIMIT_EXCEEDED",
            MnemogramError::ServiceUnavailable(_) => "SERVICE_UNAVAILABLE",
            MnemogramError::Internal(_) => "INTERNAL_ERROR",
            MnemogramError::ExternalService(_) => "EXTERNAL_SERVICE_ERROR",
            MnemogramError::Database(_) => "DATABASE_ERROR",
            MnemogramError::S3Error(_) => "S3_ERROR",
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            MnemogramError::NotFound(_) => 404,
            MnemogramError::Unauthorized(_) => 401,
            MnemogramError::Forbidden(_) => 403,
            MnemogramError::BadRequest(_) => 400,
            MnemogramError::ValidationError(_) => 400,
            MnemogramError::RateLimitExceeded(_) => 429,
            MnemogramError::ServiceUnavailable(_) => 503,
            MnemogramError::Internal(_) => 500,
            MnemogramError::ExternalService(_) => 502,
            MnemogramError::Database(_) => 500,
            MnemogramError::S3Error(_) => 500,
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
    #[serde(rename = "requestId")]
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ErrorResponse {
    pub fn new(error: &MnemogramError, request_id: String) -> Self {
        Self {
            error: error.to_string(),
            code: error.error_code().to_string(),
            request_id,
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

// Helper to convert AWS SDK errors
pub fn from_aws_error(err: &str, service: &str) -> MnemogramError {
    if err.contains("AccessDenied") {
        MnemogramError::Forbidden(format!("{} access denied", service))
    } else if err.contains("NotFound") || err.contains("ResourceNotFound") {
        MnemogramError::NotFound(format!("{} resource not found", service))
    } else if err.contains("ServiceUnavailable") {
        MnemogramError::ServiceUnavailable(format!("{} service unavailable", service))
    } else if err.contains("ValidationException") {
        MnemogramError::ValidationError(format!("{} validation failed", service))
    } else {
        MnemogramError::ExternalService(format!("{} error: {}", service, err))
    }
}
