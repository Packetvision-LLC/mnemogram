use lambda_http::{Request, RequestExt};
use serde_json::Value;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context
    let context = event.request_context();

    // For API Gateway V1, the authorizer context is available via request_context()
    // Try to extract from different possible locations based on how the authorizer returns data

    // First try: direct from Lambda authorizer context in API Gateway v2
    if let Some(authorizer) = context.authorizer.as_ref() {
        if let Some(Value::String(user_id)) = authorizer.get("userId") {
            return Ok(user_id.clone());
        }

        // Try alternative field name
        if let Some(Value::String(user_id)) = authorizer.get("user_id") {
            return Ok(user_id.clone());
        }

        // Try from custom context
        if let Some(Value::String(user_id)) = authorizer.get("principalId") {
            return Ok(user_id.clone());
        }
    }

    // Second try: from claims if using JWT authorizer
    if let Some(authorizer) = context.authorizer.as_ref() {
        if let Some(claims) = authorizer.get("claims") {
            if let Some(Value::String(user_id)) = claims.get("sub") {
                return Ok(user_id.clone());
            }
            if let Some(Value::String(user_id)) = claims.get("userId") {
                return Ok(user_id.clone());
            }
        }
    }

    Err("User ID not found in request context. Make sure the authorizer is properly configured to return 'userId' in the context.".to_string())
}
