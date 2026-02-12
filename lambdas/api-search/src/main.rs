use lambda_http::{run, service_fn, Body, Error, Request, Response};
use serde_json::json;
use tracing_subscriber::EnvFilter;

/// GET /search — hybrid BM25+vector search over .mv2 memory files.
/// Placeholder: returns 501 until MemVid integration is implemented.
async fn handler(_event: Request) -> Result<Response<Body>, Error> {
    let body = json!({
        "error": "not_implemented",
        "message": "Search endpoint is a placeholder — MemVid integration pending.",
    });

    let resp = Response::builder()
        .status(501)
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
