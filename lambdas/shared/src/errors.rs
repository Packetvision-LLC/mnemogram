use thiserror::Error;

#[derive(Error, Debug)]
pub enum MnemogramError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("internal error: {0}")]
    Internal(String),
}
