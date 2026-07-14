use thiserror::Error;

/// Errors that can occur during local embedding generation.
#[derive(Error, Debug)]
pub enum EneEmbeddingError {
    /// Error from the Candle ML inference engine (model load, forward pass, tokenizer).
    #[error("Candle ML error: {0}")]
    CandleError(String),
    /// A pre-existing typed embedding error, propagated unchanged.
    #[error(transparent)]
    Provider(#[from] crate::EmbeddingError),
}

impl From<EneEmbeddingError> for crate::EmbeddingError {
    fn from(e: EneEmbeddingError) -> Self {
        match e {
            EneEmbeddingError::CandleError(msg) => crate::EmbeddingError::Init(msg),
            EneEmbeddingError::Provider(inner) => inner,
        }
    }
}
