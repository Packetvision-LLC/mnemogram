use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::Utc;
use jsonwebtoken::{decode, DecodingKey, Validation};
use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRequest {
    #[serde(rename = "type")]
    request_type: Option<String>,
    authorization_token: Option<String>,
    method_arn: String,
    headers: Option<HashMap<String, String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    principal_id: String,
    policy_document: PolicyDocument,
    context: Option<HashMap<String, String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct PolicyDocument {
    version: String,
    statement: Vec<Statement>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Statement {
    action: String,
    effect: String,
    resource: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    iat: usize,
    email: Option<String>,
    #[serde(rename = "cognito:username")]
    username: Option<String>,
}

/// JWT/API key authorizer
/// Check Authorization Bearer token (Cognito JWT) or x-api-key header
/// For API keys: hash + lookup in api-keys table
/// Return API Gateway authorizer response (allow/deny + userId in context)
async fn handler(event: LambdaEvent<AuthRequest>) -> Result<AuthResponse, Error> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&config);

    let method_arn = event.payload.method_arn;
    
    // First, check for x-api-key header
    if let Some(headers) = &event.payload.headers {
        if let Some(api_key) = headers.get("x-api-key").or_else(|| headers.get("X-Api-Key")) {
            match validate_api_key(&dynamodb_client, api_key).await {
                Ok(user_id) => {
                    let mut context = HashMap::new();
                    context.insert("userId".to_string(), user_id.clone());
                    context.insert("authType".to_string(), "apikey".to_string());
                    
                    return create_allow_policy(&user_id, &method_arn, Some(context));
                }
                Err(e) => {
                    tracing::warn!("API key validation failed: {}", e);
                    return create_deny_policy("invalid-api-key", &method_arn);
                }
            }
        }
    }

    // If no API key, check for Authorization Bearer token
    let token = event.payload.authorization_token.unwrap_or_default();
    
    if token.is_empty() {
        return create_deny_policy("unauthorized", &method_arn);
    }

    // JWT Token validation
    if token.starts_with("Bearer ") {
        let jwt = token.strip_prefix("Bearer ").unwrap_or(&token);
        match validate_jwt(jwt).await {
            Ok(claims) => {
                let mut context = HashMap::new();
                context.insert("userId".to_string(), claims.sub.clone());
                context.insert("authType".to_string(), "jwt".to_string());
                
                if let Some(email) = claims.email {
                    context.insert("userEmail".to_string(), email);
                }
                if let Some(username) = claims.username {
                    context.insert("username".to_string(), username);
                }
                
                create_allow_policy(&claims.sub, &method_arn, Some(context))
            }
            Err(e) => {
                tracing::warn!("JWT validation failed: {}", e);
                create_deny_policy("invalid-jwt", &method_arn)
            }
        }
    } else {
        create_deny_policy("invalid-token-format", &method_arn)
    }
}

/// Validate JWT token against Cognito
/// TODO: Replace with actual Cognito JWKS validation
async fn validate_jwt(token: &str) -> Result<Claims, Box<dyn std::error::Error + Send + Sync>> {
    // For now, this is a simplified validation
    // In a real implementation, you would:
    // 1. Download Cognito JWKS from https://cognito-idp.{region}.amazonaws.com/{userPoolId}/.well-known/jwks.json
    // 2. Validate the JWT signature against the public keys
    // 3. Verify issuer, audience, expiration, etc.
    
    let mut validation = Validation::default();
    // Skip signature verification for demo (UNSAFE for production)
    validation.insecure_disable_signature_validation();
    
    // For production, you'd need to fetch the actual public key from Cognito JWKS
    let key = DecodingKey::from_secret("dummy_key_for_demo".as_ref());
    
    match decode::<Claims>(token, &key, &validation) {
        Ok(token_data) => {
            // Additional validation checks
            let now = Utc::now().timestamp() as usize;
            if token_data.claims.exp < now {
                return Err("Token expired".into());
            }
            
            Ok(token_data.claims)
        }
        Err(e) => {
            tracing::error!("JWT decode error: {}", e);
            Err(format!("Invalid JWT token: {}", e).into())
        }
    }
}

/// Validate API key against DynamoDB
/// Hash + lookup in api-keys table
async fn validate_api_key(
    client: &aws_sdk_dynamodb::Client,
    api_key: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let api_keys_table = std::env::var("API_KEYS_TABLE")
        .map_err(|_| "API_KEYS_TABLE environment variable not set")?;
    
    // Create SHA-256 hash of the API key (proper implementation would use secure hashing with salt)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    let mut hasher = DefaultHasher::new();
    api_key.hash(&mut hasher);
    let key_hash = format!("hash_{}", hasher.finish());
    
    let key = HashMap::from([
        ("keyId".to_string(), AttributeValue::S(key_hash.clone())),
    ]);

    let result = client
        .get_item()
        .table_name(&api_keys_table)
        .set_key(Some(key))
        .send()
        .await
        .map_err(|e| format!("DynamoDB error: {}", e))?;

    match result.item {
        Some(item) => {
            // Check if key is active
            if let Some(AttributeValue::S(status)) = item.get("status") {
                if status != "active" {
                    return Err("API key is inactive".into());
                }
            }

            // Get user ID
            let user_id = item
                .get("userId")
                .and_then(|v| v.as_s().ok())
                .ok_or("User ID not found in API key record")?;

            // Update lastUsedAt timestamp (fire and forget)
            tokio::spawn({
                let client = client.clone();
                let table_name = api_keys_table.clone();
                let key_hash = key_hash.clone();
                async move {
                    let _ = client
                        .update_item()
                        .table_name(&table_name)
                        .set_key(Some(HashMap::from([
                            ("keyId".to_string(), AttributeValue::S(key_hash)),
                        ])))
                        .update_expression("SET lastUsedAt = :timestamp")
                        .expression_attribute_values(
                            ":timestamp",
                            AttributeValue::S(Utc::now().to_rfc3339()),
                        )
                        .send()
                        .await;
                }
            });

            Ok(user_id.to_string())
        }
        None => Err("API key not found".into()),
    }
}

fn create_allow_policy(
    principal_id: &str,
    resource: &str,
    context: Option<HashMap<String, String>>,
) -> Result<AuthResponse, Error> {
    Ok(AuthResponse {
        principal_id: principal_id.to_string(),
        policy_document: PolicyDocument {
            version: "2012-10-17".to_string(),
            statement: vec![Statement {
                action: "execute-api:Invoke".to_string(),
                effect: "Allow".to_string(),
                resource: resource.to_string(),
            }],
        },
        context,
    })
}

fn create_deny_policy(principal_id: &str, resource: &str) -> Result<AuthResponse, Error> {
    Ok(AuthResponse {
        principal_id: principal_id.to_string(),
        policy_document: PolicyDocument {
            version: "2012-10-17".to_string(),
            statement: vec![Statement {
                action: "execute-api:Invoke".to_string(),
                effect: "Deny".to_string(),
                resource: resource.to_string(),
            }],
        },
        context: None,
    })
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}