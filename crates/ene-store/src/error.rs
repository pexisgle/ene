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

    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}

/// Type alias for internal module usages.
pub type MemoryError = EneMemoryError;
