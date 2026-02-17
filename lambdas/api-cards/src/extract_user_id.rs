use lambda_http::{Request, RequestExt};
use serde_json::Value;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the authorizer context directly
    if let Some(authorizer) = event.request_context().authorizer() {
        // Try to get userId from authorizer context
        if let Some(Value::String(user_id)) = authorizer.fields.get("userId") {
            return Ok(user_id.clone());
        }

        // Also try different key formats that might be used
        if let Some(Value::String(user_id)) = authorizer.fields.get("user_id") {
            return Ok(user_id.clone());
        }

        if let Some(Value::String(user_id)) = authorizer.fields.get("sub") {
            return Ok(user_id.clone());
        }
    }

    Err(
        "User ID not found in request context. Make sure the authorizer is properly configured."
            .to_string(),
    )
}
