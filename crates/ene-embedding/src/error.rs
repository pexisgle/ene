use thiserror::Error;

/// Errors that can occur during embedding generation.
#[derive(Error, Debug)]
pub enum EneEmbeddingError {
    /// Error from the Candle ML inference engine (model load, forward pass, tokenizer).
    #[error("Candle ML error: {0}")]
    CandleError(String),
    /// A pre-existing typed embedding error, propagated unchanged.
    #[error(transparent)]
    Provider(#[from] ene_provider::EmbeddingError),
}

/// Type alias for internal module usages.
pub type EmbeddingError = EneEmbeddingError;

impl From<EneEmbeddingError> for ene_provider::EmbeddingError {
    fn from(e: EneEmbeddingError) -> Self {
        match e {
            EneEmbeddingError::CandleError(msg) => ene_provider::EmbeddingError::Init(msg),
            // Propagate typed errors unchanged (thiserror's `#[from]` only
            // applies one direction, so we re-wrap here).
            EneEmbeddingError::Provider(inner) => inner,
        }
    }
}
