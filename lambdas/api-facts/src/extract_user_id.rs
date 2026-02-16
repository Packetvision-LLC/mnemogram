use lambda_http::{Request, RequestExt, RequestContext};
use serde_json::Value;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context
    let context = event.request_context();
    
    // Extract authorizer context from request context using the new API
    match context {
        RequestContext::ApiGatewayV1(proxy_context) => {
            // Try to get userId from the authorizer fields
            if let Some(user_id_value) = proxy_context.authorizer.fields.get("userId") {
                if let Some(user_id_str) = user_id_value.as_str() {
                    return Ok(user_id_str.to_string());
                }
            }
        }
        RequestContext::ApiGatewayV2(v2_context) => {
            // For API Gateway V2, check if there's an authorizer context
            if let Some(authorizer) = &v2_context.authorizer {
                if let Some(user_id_value) = authorizer.get("userId") {
                    if let Some(user_id_str) = user_id_value.as_str() {
                        return Ok(user_id_str.to_string());
                    }
                }
            }
        }
        RequestContext::WebSocket(ws_context) => {
            // Try to get userId from the authorizer fields
            if let Some(user_id_value) = ws_context.authorizer.get("userId") {
                if let Some(user_id_str) = user_id_value.as_str() {
                    return Ok(user_id_str.to_string());
                }
            }
        }
        RequestContext::Alb(_) => {
            // ALB doesn't typically have authorizer context
        }
    }
    
    Err("User ID not found in request context. Make sure the authorizer is properly configured.".to_string())
}