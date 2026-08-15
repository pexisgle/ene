use thiserror::Error;

use ene_ai::EmbeddingError;

#[derive(Error, Debug)]
pub enum EneEmbeddingError {
    /// Error from the local llama.cpp embedding path (model load, forward, tokenize).
    #[error("local embedding error: {0}")]
    LocalLlm(String),
    /// A pre-existing typed embedding error, propagated unchanged.
    #[error(transparent)]
    Provider(#[from] EmbeddingError),
}

impl From<EneEmbeddingError> for EmbeddingError {
    fn from(e: EneEmbeddingError) -> Self {
        match e {
            EneEmbeddingError::LocalLlm(msg) => Self::Init(msg),
            EneEmbeddingError::Provider(inner) => inner,
        }
    }
}

impl From<ene_ai::error::LlmProviderError> for EneEmbeddingError {
    fn from(e: ene_ai::error::LlmProviderError) -> Self {
        use ene_ai::error::LlmProviderError;
        match e {
            LlmProviderError::LocalLlm(msg) => Self::LocalLlm(msg),
            LlmProviderError::Auth(msg) => {
                Self::Provider(EmbeddingError::Provider(format!("auth: {msg}")))
            }
            LlmProviderError::RateLimit(msg) => {
                Self::Provider(EmbeddingError::Provider(format!("rate limit: {msg}")))
            }
            LlmProviderError::Network(msg) => {
                Self::Provider(EmbeddingError::Provider(format!("network: {msg}")))
            }
            other => Self::LocalLlm(other.to_string()),
        }
    }
}
