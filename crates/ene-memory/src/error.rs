use thiserror::Error;

/// Error types for the memory subsystem.
#[derive(Error, Debug)]
pub enum MemoryError {
    /// Missing base URL for API calls.
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl {
        /// The environment variable name for the base URL.
        env_var: String
    },
    /// Diesel/sqlite error from the memory store.
    #[error("Memory store error: {0}")]
    MemoryStoreError(#[from] diesel::result::Error),
    /// Failed to connect to the memory store.
    #[error("Memory store connection error: {0}")]
    MemoryStoreConnectionError(String),
    /// Failed to build a prompt for summarization.
    #[error("Prompt building failed: {0}")]
    PromptBuildError(String),
    /// API request failed.
    #[error("API request failed: {0}")]
    ApiRequestError(String),
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
    /// Embedding error.
    #[error(transparent)]
    Embedding(#[from] ene_embedding::error::EmbeddingError),
    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}
