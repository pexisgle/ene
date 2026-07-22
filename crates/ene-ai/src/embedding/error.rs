use thiserror::Error;

/// Errors that can occur during local embedding generation.
#[derive(Error, Debug)]
pub enum EneEmbeddingError {
    /// Error from the local llama.cpp embedding path (model load, forward, tokenize).
    #[error("local embedding error: {0}")]
    LocalLlm(String),
    /// A pre-existing typed embedding error, propagated unchanged.
    #[error(transparent)]
    Provider(#[from] crate::EmbeddingError),
}

impl From<EneEmbeddingError> for crate::EmbeddingError {
    fn from(e: EneEmbeddingError) -> Self {
        match e {
            EneEmbeddingError::LocalLlm(msg) => Self::Init(msg),
            EneEmbeddingError::Provider(inner) => inner,
        }
    }
}

impl From<crate::error::LlmProviderError> for EneEmbeddingError {
    fn from(e: crate::error::LlmProviderError) -> Self {
        match e {
            crate::error::LlmProviderError::LocalLlm(msg) => Self::LocalLlm(msg),
            crate::error::LlmProviderError::Auth(msg) => {
                Self::Provider(crate::EmbeddingError::Provider(format!("auth: {msg}")))
            }
            crate::error::LlmProviderError::RateLimit(msg) => Self::Provider(
                crate::EmbeddingError::Provider(format!("rate limit: {msg}")),
            ),
            crate::error::LlmProviderError::Network(msg) => {
                Self::Provider(crate::EmbeddingError::Provider(format!("network: {msg}")))
            }
            other => Self::LocalLlm(other.to_string()),
        }
    }
}
