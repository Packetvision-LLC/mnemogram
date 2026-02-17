use aws_sdk_dynamodb::Client as DynamoClient;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct AuthorizerEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(rename = "authorizationToken")]
    authorization_token: Option<String>,
    #[serde(rename = "methodArn")]
    method_arn: String,
    #[serde(rename = "requestContext")]
    request_context: Value,
    headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct AuthorizerResponse {
    #[serde(rename = "principalId")]
    principal_id: String,
    #[serde(rename = "policyDocument")]
    policy_document: PolicyDocument,
    context: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
struct PolicyDocument {
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Statement")]
    statement: Vec<PolicyStatement>,
}

#[derive(Debug, Serialize)]
struct PolicyStatement {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Effect")]
    effect: String,
    #[serde(rename = "Resource")]
    resource: String,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    subscription_tier: Option<String>,
    subscription_status: Option<String>,
    exp: usize,
}

#[derive(Debug, Deserialize)]
struct UserRecord {
    user_id: String,
    email: String,
    subscription_tier: String,
    subscription_status: String,
    api_key: String,
    rate_limit_tier: String,
}

async fn handler(event: LambdaEvent<AuthorizerEvent>) -> Result<AuthorizerResponse, Error> {
    let (event, _context) = event.into_parts();

    // Extract API key from Authorization header or query parameter
    let api_key = extract_api_key(&event)?;

    // Initialize DynamoDB client
    let config = aws_config::load_from_env().await;
    let dynamodb_client = DynamoClient::new(&config);

    // Validate API key and get user subscription info
    let user = validate_api_key_and_subscription(&dynamodb_client, &api_key).await?;

    // Check subscription status
    if user.subscription_status != "active" {
        return Err("Subscription not active".into());
    }

    // Generate policy based on subscription tier
    let policy = generate_policy(&event.method_arn, &user.subscription_tier);

    // Create context with user info and rate limits
    let mut context = HashMap::new();
    context.insert("userId".to_string(), user.user_id.clone());
    context.insert("email".to_string(), user.email.clone());
    context.insert(
        "subscriptionTier".to_string(),
        user.subscription_tier.clone(),
    );
    context.insert("rateLimitTier".to_string(), user.rate_limit_tier.clone());

    Ok(AuthorizerResponse {
        principal_id: user.user_id,
        policy_document: policy,
        context,
    })
}

fn extract_api_key(event: &AuthorizerEvent) -> Result<String, Error> {
    // Try Authorization header first (Bearer token format)
    if let Some(token) = &event.authorization_token {
        if token.starts_with("Bearer ") {
            return Ok(token.replace("Bearer ", ""));
        }
        return Ok(token.clone());
    }

    // Try x-api-key header
    if let Some(headers) = &event.headers {
        if let Some(api_key) = headers.get("x-api-key") {
            return Ok(api_key.clone());
        }
        if let Some(api_key) = headers.get("X-API-Key") {
            return Ok(api_key.clone());
        }
    }

    Err("No API key found in request".into())
}

async fn validate_api_key_and_subscription(
    client: &DynamoClient,
    api_key: &str,
) -> Result<UserRecord, Error> {
    let table_name =
        std::env::var("USERS_TABLE").unwrap_or_else(|_| "mnemogram-dev-users".to_string());

    // Query DynamoDB for user by API key (GSI lookup)
    let result = client
        .query()
        .table_name(&table_name)
        .index_name("api-key-index")
        .key_condition_expression("api_key = :api_key")
        .expression_attribute_values(
            ":api_key",
            aws_sdk_dynamodb::types::AttributeValue::S(api_key.to_string()),
        )
        .send()
        .await
        .map_err(|e| format!("DynamoDB query failed: {}", e))?;

    let items = result.items.as_deref().unwrap_or(&[]);
    if items.is_empty() {
        return Err("Invalid API key".into());
    }

    let item = &items[0];

    // Extract user data from DynamoDB item
    let user_id = item
        .get("user_id")
        .and_then(|v: &aws_sdk_dynamodb::types::AttributeValue| v.as_s().ok())
        .ok_or("Missing user_id")?;

    let email = item
        .get("email")
        .and_then(|v: &aws_sdk_dynamodb::types::AttributeValue| v.as_s().ok())
        .ok_or("Missing email")?;

    let subscription_tier = item
        .get("subscription_tier")
        .and_then(|v: &aws_sdk_dynamodb::types::AttributeValue| v.as_s().ok())
        .map_or("free", |s| s.as_str());

    let subscription_status = item
        .get("subscription_status")
        .and_then(|v: &aws_sdk_dynamodb::types::AttributeValue| v.as_s().ok())
        .map_or("inactive", |s| s.as_str());

    let rate_limit_tier = match subscription_tier {
        "enterprise" => "enterprise", // 1000 req/sec
        "pro" => "pro",               // 100 req/sec
        _ => "free",                  // 10 req/sec
    };

    Ok(UserRecord {
        user_id: user_id.to_string(),
        email: email.to_string(),
        subscription_tier: subscription_tier.to_string(),
        subscription_status: subscription_status.to_string(),
        api_key: api_key.to_string(),
        rate_limit_tier: rate_limit_tier.to_string(),
    })
}

fn generate_policy(method_arn: &str, subscription_tier: &str) -> PolicyDocument {
    let effect = "Allow";

    // Parse ARN to create resource pattern
    let resource = if method_arn.contains("/v1/") {
        // Allow all v1 endpoints for active subscribers
        method_arn.replace("/v1/*", "/v1/*")
    } else {
        method_arn.to_string()
    };

    // Enterprise tier gets additional access
    let mut statements = vec![PolicyStatement {
        action: "execute-api:Invoke".to_string(),
        effect: effect.to_string(),
        resource: resource.clone(),
    }];

    if subscription_tier == "enterprise" {
        // Enterprise gets access to admin endpoints
        statements.push(PolicyStatement {
            action: "execute-api:Invoke".to_string(),
            effect: effect.to_string(),
            resource: method_arn.replace("/v1/", "/admin/"),
        });
    }

    PolicyDocument {
        version: "2012-10-17".to_string(),
        statement: statements,
    }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    lambda_runtime::run(service_fn(handler)).await
}
