use thiserror::Error;

/// Common error type shared across the ene workspace.
#[derive(Error, Debug)]
pub enum CommonError {
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Embedding error.
    #[error("Embedding error: {0}")]
    Embedding(String),

    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}
