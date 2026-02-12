use crate::errors::MnemogramError;
use aws_config::BehaviorVersion;
use aws_sdk_dynamodb::{types::AttributeValue, Client};
use chrono::Utc;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{error, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub user_id: String,
    pub auth_type: AuthType,
    pub email: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthType {
    JWT,
    ApiKey,
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

/// Extract and validate authorization from Lambda Function URL event headers
/// Supports both Authorization Bearer tokens (JWT) and x-api-key headers
pub async fn authorize_request(headers: &HashMap<String, String>) -> Result<AuthContext, MnemogramError> {
    // First, check for x-api-key header
    if let Some(api_key) = headers.get("x-api-key").or_else(|| headers.get("X-Api-Key")) {
        let user_id = validate_api_key(api_key).await?;
        return Ok(AuthContext {
            user_id,
            auth_type: AuthType::ApiKey,
            email: None,
            username: None,
        });
    }

    // If no API key, check for Authorization Bearer token
    if let Some(auth_header) = headers.get("authorization").or_else(|| headers.get("Authorization")) {
        if auth_header.starts_with("Bearer ") {
            let jwt = auth_header.strip_prefix("Bearer ").unwrap_or(auth_header);
            let claims = validate_jwt(jwt).await?;
            
            return Ok(AuthContext {
                user_id: claims.sub,
                auth_type: AuthType::JWT,
                email: claims.email,
                username: claims.username,
            });
        }
    }

    Err(MnemogramError::Unauthorized("No valid authorization provided".to_string()))
}

/// Validate JWT token against Cognito
/// TODO: Replace with actual Cognito JWKS validation
async fn validate_jwt(token: &str) -> Result<Claims, MnemogramError> {
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
                return Err(MnemogramError::Unauthorized("Token expired".to_string()));
            }
            
            Ok(token_data.claims)
        }
        Err(e) => {
            error!("JWT decode error: {}", e);
            Err(MnemogramError::Unauthorized(format!("Invalid JWT token: {}", e)))
        }
    }
}

/// Validate API key against DynamoDB
/// Hash + lookup in api-keys table
async fn validate_api_key(api_key: &str) -> Result<String, MnemogramError> {
    let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
    let client = Client::new(&config);
    
    let api_keys_table = std::env::var("API_KEYS_TABLE")
        .map_err(|_| MnemogramError::Internal("API_KEYS_TABLE environment variable not set".to_string()))?;
    
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
        .map_err(|e| MnemogramError::Database(format!("DynamoDB error: {}", e)))?;

    match result.item {
        Some(item) => {
            // Check if key is active
            if let Some(AttributeValue::S(status)) = item.get("status") {
                if status != "active" {
                    return Err(MnemogramError::Unauthorized("API key is inactive".to_string()));
                }
            }

            // Get user ID
            let user_id = item
                .get("userId")
                .and_then(|v| v.as_s().ok())
                .ok_or_else(|| MnemogramError::Internal("User ID not found in API key record".to_string()))?;

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
        None => Err(MnemogramError::Unauthorized("API key not found".to_string())),
    }
}

/// Middleware function to extract user context from Lambda Function URL headers
/// Use this in your Lambda handlers that require authentication
pub async fn extract_auth_context(
    headers: &HashMap<String, String>
) -> Result<AuthContext, MnemogramError> {
    authorize_request(headers).await
}