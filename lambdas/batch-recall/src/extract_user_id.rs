use lambda_http::{Request, RequestExt};
use lambda_http::request::RequestContext;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context
    let context = event.request_context();
    
    // Try to extract user_id from different authorizer patterns
    match context {
        RequestContext::ApiGatewayV1(ctx) => {
            if let Some(authorizer) = &ctx.authorizer {
                if let Some(user_id) = authorizer.get("userId") {
                    if let Some(user_id_str) = user_id.as_str() {
                        return Ok(user_id_str.to_string());
                    }
                }
            }
        }
        RequestContext::ApiGatewayV2(ctx) => {
            if let Some(authorizer) = &ctx.authorizer {
                if let Some(lambda_context) = &authorizer.lambda {
                    if let Some(user_id) = lambda_context.get("userId") {
                        if let Some(user_id_str) = user_id.as_str() {
                            return Ok(user_id_str.to_string());
                        }
                    }
                }
            }
        }
        RequestContext::Alb(_) => {
            // ALB doesn't typically have authorizer context
            return Err("ALB context doesn't support authorizer data".to_string());
        }
    }
    
    Err("User ID not found in request context. Make sure the authorizer is properly configured.".to_string())
}
