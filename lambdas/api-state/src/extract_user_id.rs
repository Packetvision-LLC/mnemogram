use lambda_http::{Request, RequestExt};

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Try to get user ID from headers first (common pattern)
    if let Some(user_id) = event.headers().get("x-user-id") {
        if let Ok(user_id_str) = user_id.to_str() {
            return Ok(user_id_str.to_string());
        }
    }

    // Try to extract from request context as JSON
    let context = event.request_context();

    // Serialize the context to JSON to work around type issues
    if let Ok(context_json) = serde_json::to_value(&context) {
        // Try different paths where user ID might be stored
        let possible_paths: Vec<Vec<&str>> = vec![
            vec!["authorizer", "userId"],
            vec!["authorizer", "fields", "userId"],
            vec!["authorizer", "principalId"],
        ];

        for path in &possible_paths {
            let mut current = &context_json;
            for segment in path {
                if let Some(value) = current.get(segment) {
                    current = value;
                } else {
                    break;
                }
            }

            if let Some(user_id_str) = current.as_str() {
                return Ok(user_id_str.to_string());
            }
        }
    }

    Err(
        "User ID not found in request context. Make sure the authorizer is properly configured."
            .to_string(),
    )
}
