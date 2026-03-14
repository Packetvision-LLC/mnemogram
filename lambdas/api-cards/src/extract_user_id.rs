use lambda_http::{request::ApiGatewayRequestAuthorizer, Request, RequestExt};

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    let context = event.request_context();

    if let Some(authorizer) = context.authorizer() {
        if let Some(user_id) = extract_user_id_from_authorizer(authorizer) {
            return Ok(user_id);
        }
    }

    Err(
        "User ID not found in request context. Make sure the authorizer is properly configured."
            .to_string(),
    )
}

fn extract_user_id_from_authorizer(authorizer: &ApiGatewayRequestAuthorizer) -> Option<String> {
    authorizer
        .fields
        .get("userId")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .or_else(|| {
            authorizer
                .jwt
                .as_ref()
                .and_then(|jwt| jwt.claims.get("userId").cloned())
        })
        .or_else(|| authorizer.iam.as_ref().and_then(|iam| iam.user_id.clone()))
}
