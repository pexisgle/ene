use thiserror::Error;

#[derive(Error, Debug)]
pub enum MemoryError {
    #[error("Missing base url: set {env_var} or configure AI Base URL")]
    MissingBaseUrl { env_var: String },
    #[error("Memory store error: {0}")]
    MemoryStoreError(#[from] diesel::result::Error),
    #[error("Memory store connection error: {0}")]
    MemoryStoreConnectionError(String),
    #[error("Prompt building failed: {0}")]
    PromptBuildError(String),
    #[error("API request failed: {0}")]
    ApiRequestError(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error(transparent)]
    Embedding(#[from] ene_embedding::error::EmbeddingError),
    #[error("Other error: {0}")]
    Other(String),
}
