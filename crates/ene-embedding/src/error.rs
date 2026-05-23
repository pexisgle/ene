use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmbeddingError {
    #[error("Embedding error: {0}")]
    EmbeddingError(String),
    #[error("Candle ML error: {0}")]
    CandleError(String),
}
