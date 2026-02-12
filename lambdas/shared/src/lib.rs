pub mod errors;
pub mod middleware;
pub mod logging;
pub mod validation;
pub mod auth;    // JWT/API key validation for Function URLs
pub mod mv2_cache;  // .mv2 file caching for Lambda /tmp

// Future modules:
// pub mod s3;      // S3 get/put helpers
// pub mod dynamo;  // DynamoDB helpers
