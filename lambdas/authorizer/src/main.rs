use lambda_runtime::{run, service_fn, Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthRequest {
    authorization_token: Option<String>,
    method_arn: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthResponse {
    principal_id: String,
    policy_document: PolicyDocument,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyDocument {
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Statement")]
    statement: Vec<Statement>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Statement {
    #[serde(rename = "Action")]
    action: String,
    #[serde(rename = "Effect")]
    effect: String,
    #[serde(rename = "Resource")]
    resource: String,
}

/// Custom authorizer — validates Cognito JWTs.
/// Placeholder: currently denies all requests.
async fn handler(event: LambdaEvent<AuthRequest>) -> Result<AuthResponse, Error> {
    let arn = event.payload.method_arn.unwrap_or_default();

    // TODO: Validate JWT from event.payload.authorization_token against Cognito JWKS
    let _token = event.payload.authorization_token.unwrap_or_default();

    Ok(AuthResponse {
        principal_id: "anonymous".to_string(),
        policy_document: PolicyDocument {
            version: "2012-10-17".to_string(),
            statement: vec![Statement {
                action: "execute-api:Invoke".to_string(),
                effect: "Deny".to_string(),
                resource: arn,
            }],
        },
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
