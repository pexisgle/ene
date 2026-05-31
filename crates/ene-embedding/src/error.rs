use thiserror::Error;

/// Errors that can occur during embedding generation.
#[derive(Error, Debug)]
pub enum EneEmbeddingError {
    /// General embedding error.
    #[error("Embedding error: {0}")]
    EmbeddingError(String),
    /// Error from the Candle ML inference engine.
    #[error("Candle ML error: {0}")]
    CandleError(String),
}

/// Type alias for internal module usages.
pub type EmbeddingError = EneEmbeddingError;
