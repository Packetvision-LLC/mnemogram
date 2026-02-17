use lambda_http::{Request, RequestExt};
use serde_json::Value;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context
    let context = event.request_context();

    // Extract authorizer context from request context
    if let Some(authorizer) = context.get("authorizer") {
        if let Some(user_id) = authorizer.get("userId") {
            if let Some(user_id_str) = user_id.as_str() {
                return Ok(user_id_str.to_string());
            }
        }
    }

    // Fallback: try to get from custom authorizer context
    if let Some(Value::Object(context_map)) = context.get("authorizer") {
        if let Some(Value::String(user_id)) = context_map.get("userId") {
            return Ok(user_id.clone());
        }
    }

    Err(
        "User ID not found in request context. Make sure the authorizer is properly configured."
            .to_string(),
    )
}
