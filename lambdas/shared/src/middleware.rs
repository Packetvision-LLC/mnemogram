use crate::errors::{ErrorResponse, MnemogramError};
use lambda_runtime::{Context, LambdaEvent};
use serde_json::Value;
use std::future::Future;
use tracing::{error, info, warn};

/// Middleware that wraps Lambda handlers to provide consistent error handling and logging
pub async fn handle_with_middleware<F, Fut, I, O>(
    event: LambdaEvent<I>,
    handler: F,
) -> Result<O, lambda_runtime::Error>
where
    F: FnOnce(I, Context) -> Fut,
    Fut: Future<Output = Result<O, MnemogramError>>,
    I: serde::de::DeserializeOwned,
    O: serde::Serialize,
{
    let context = event.context.clone();
    let request_id = context.request_id.clone();
    
    // Log request start
    info!(
        request_id = %request_id,
        function_name = %context.env_config.function_name,
        "Request started"
    );

    // Execute the handler
    match handler(event.payload, context).await {
        Ok(response) => {
            info!(request_id = %request_id, "Request completed successfully");
            Ok(response)
        }
        Err(err) => {
            // Log the error with appropriate level
            match err {
                MnemogramError::Internal(_) 
                | MnemogramError::Database(_) 
                | MnemogramError::S3Error(_) 
                | MnemogramError::ExternalService(_) => {
                    error!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Internal server error"
                    );
                }
                MnemogramError::Unauthorized(_) | MnemogramError::Forbidden(_) => {
                    warn!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Authorization error"
                    );
                }
                _ => {
                    info!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Client error"
                    );
                }
            }

            // Create error response
            let error_response = ErrorResponse::new(&err, request_id);
            let error_json = serde_json::to_value(error_response)
                .unwrap_or_else(|_| serde_json::json!({
                    "error": "Failed to serialize error response",
                    "code": "SERIALIZATION_ERROR",
                    "requestId": request_id
                }));

            // Return as Lambda error with proper status code
            Err(lambda_runtime::Error::from(format!(
                "Status: {}, Body: {}",
                err.status_code(),
                error_json
            )))
        }
    }
}

/// HTTP-specific middleware for API Gateway Lambda handlers
#[cfg(feature = "http")]
pub async fn handle_http_with_middleware<F, Fut, I>(
    event: LambdaEvent<lambda_http::Request>,
    handler: F,
) -> Result<lambda_http::Response<lambda_http::Body>, lambda_runtime::Error>
where
    F: FnOnce(lambda_http::Request, Context) -> Fut,
    Fut: Future<Output = Result<I, MnemogramError>>,
    I: serde::Serialize,
{
    use lambda_http::{Body, Response};
    use std::collections::HashMap;

    let context = event.context.clone();
    let request_id = context.request_id.clone();
    
    // Log request start with HTTP details
    info!(
        request_id = %request_id,
        method = %event.payload.method(),
        path = %event.payload.uri().path(),
        "HTTP request started"
    );

    // Execute the handler
    match handler(event.payload, context).await {
        Ok(response) => {
            info!(request_id = %request_id, "HTTP request completed successfully");
            
            let body = serde_json::to_string(&response)
                .unwrap_or_else(|_| r#"{"error": "Serialization failed"}"#.to_string());
                
            Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .header("X-Request-ID", request_id)
                .body(Body::Text(body))
                .map_err(|e| lambda_runtime::Error::from(format!("Response building failed: {}", e)))
        }
        Err(err) => {
            // Log the error
            match err {
                MnemogramError::Internal(_) 
                | MnemogramError::Database(_) 
                | MnemogramError::S3Error(_) 
                | MnemogramError::ExternalService(_) => {
                    error!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Internal server error"
                    );
                }
                MnemogramError::Unauthorized(_) | MnemogramError::Forbidden(_) => {
                    warn!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Authorization error"
                    );
                }
                _ => {
                    info!(
                        request_id = %request_id,
                        error = %err,
                        error_code = err.error_code(),
                        "Client error"
                    );
                }
            }

            // Create error response
            let error_response = ErrorResponse::new(&err, request_id.clone());
            let error_json = serde_json::to_string(&error_response)
                .unwrap_or_else(|_| r#"{"error": "Serialization failed", "code": "SERIALIZATION_ERROR"}"#.to_string());

            Response::builder()
                .status(err.status_code())
                .header("Content-Type", "application/json")
                .header("X-Request-ID", request_id)
                .body(Body::Text(error_json))
                .map_err(|e| lambda_runtime::Error::from(format!("Error response building failed: {}", e)))
        }
    }
}

/// Utility to extract request ID from various contexts
pub fn get_request_id(context: &Context) -> String {
    context.request_id.clone()
}

/// CORS headers helper
#[cfg(feature = "http")]
pub fn add_cors_headers(mut response: lambda_http::Response<lambda_http::Body>) -> lambda_http::Response<lambda_http::Body> {
    let headers = response.headers_mut();
    headers.insert("Access-Control-Allow-Origin", "*".parse().unwrap());
    headers.insert("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS".parse().unwrap());
    headers.insert("Access-Control-Allow-Headers", "Content-Type, Authorization, X-API-Version".parse().unwrap());
    headers.insert("Access-Control-Max-Age", "86400".parse().unwrap());
    response
}