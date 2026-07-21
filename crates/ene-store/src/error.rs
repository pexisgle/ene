use thiserror::Error;

/// Error types for the memory subsystem.
#[derive(Error, Debug)]
pub enum EneMemoryError {
    /// `SeaORM` error from the memory store.
    #[error("Memory store error: {0}")]
    MemoryStoreError(#[from] sea_orm::DbErr),
    /// Failed to connect to the memory store.
    #[error("Memory store connection error: {0}")]
    MemoryStoreConnectionError(String),
    /// API request failed.
    #[error("API request failed: {0}")]
    ApiRequestError(String),
    /// Embedding failed structural validation: wrong length
    /// vs. the store's `embedding_dim`, contains a NaN or
    /// Infinity, or is otherwise unusable for cosine
    /// similarity.
    #[error("Invalid embedding: {0}")]
    InvalidEmbedding(String),

    /// Memory lifecycle status transition is not permitted.
    #[error("Invalid memory status transition: {from:?} -> {to:?}")]
    InvalidTransition {
        /// Current status.
        from: crate::MemoryStatus,
        /// Requested target status.
        to: crate::MemoryStatus,
    },

    /// Session export uses an unsupported `format_version`.
    #[error("Unsupported session export format version: {0}")]
    UnsupportedFormatVersion(u32),

    /// Failed to (de)serialize a session export payload.
    #[error("Session export serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}

/// Type alias for internal module usages.
pub type MemoryError = EneMemoryError;
