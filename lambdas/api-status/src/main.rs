use lambda_http::{run, service_fn, Body, Error, Request, Response};
use serde_json::json;
use tracing_subscriber::EnvFilter;

/// Health check endpoint — returns service status and version.
async fn handler(_event: Request) -> Result<Response<Body>, Error> {
    let body = json!({
        "status": "ok",
        "service": "mnemogram",
        "version": env!("CARGO_PKG_VERSION"),
    });

    let resp = Response::builder()
        .status(200)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body)?))
        .map_err(Box::new)?;

    Ok(resp)
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .json()
        .init();

    run(service_fn(handler)).await
}
