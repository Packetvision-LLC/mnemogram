use lambda_http::{Request, RequestExt};
use lambda_http::request::RequestContext;

pub fn extract_user_id_from_context(event: &Request) -> Result<String, String> {
    // Get the request context
    let context = event.request_context();
    
    // Extract authorizer context from request context
    match context {
        RequestContext::ApiGatewayV1(ctx) => {
            if let Some(authorizer) = &ctx.authorizer {
                if let Some(user_id) = authorizer.get("userId") {
                    if let Some(user_id_str) = user_id.as_str() {
                        return Ok(user_id_str.to_string());
                    }
                }
            }
        },
        RequestContext::ApiGatewayV2(ctx) => {
            if let Some(authorizer) = &ctx.authorizer {
                if let Some(lambda) = &authorizer.lambda {
                    if let Some(user_id) = lambda.get("userId") {
                        if let Some(user_id_str) = user_id.as_str() {
                            return Ok(user_id_str.to_string());
                        }
                    }
                }
            }
        },
        RequestContext::Alb(_) => {
            return Err("ALB context not supported for user extraction".to_string());
        },
    }
    
    Err("User ID not found in request context. Make sure the authorizer is properly configured.".to_string())
}
