use lambda_http::{Request, RequestExt};
use serde_json::Value;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context and extract from authorizer
    let context = event.request_context();

    // Extract userId from the authorizer context
    if let Some(Value::String(user_id)) = context.authorizer().get("userId") {
        return Ok(user_id.clone());
    }

    // Fallback: try other possible locations in authorizer context
    if let Some(authorizer_map) = context.authorizer().as_object() {
        if let Some(Value::String(user_id)) = authorizer_map.get("userId") {
            return Ok(user_id.clone());
        }
        // Try alternative key names
        if let Some(Value::String(user_id)) = authorizer_map.get("user_id") {
            return Ok(user_id.clone());
        }
        if let Some(Value::String(user_id)) = authorizer_map.get("sub") {
            return Ok(user_id.clone());
        }
    }

    Err(
        "User ID not found in request context. Make sure the authorizer is properly configured."
            .to_string(),
    )
}
