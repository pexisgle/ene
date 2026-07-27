use thiserror::Error;

/// Error types for the memory subsystem.
///
/// Marked `#[non_exhaustive]` so downstream crates (notably `ene-runtime`'s
/// `public_api::PublicApiError` boundary, #269) cannot exhaustively match
/// this enum without a wildcard arm. That means adding a new variant here
/// never breaks a downstream crate's compile — new variants silently fall
/// through any wildcard arm until that crate chooses to special-case them.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum EneMemoryError {
    /// `SeaORM` error from the memory store.
    #[error("Memory store error: {0}")]
    MemoryStoreError(#[from] sea_orm::DbErr),
    /// Failed to connect to the memory store.
    #[error("Memory store connection error: {0}")]
    MemoryStoreConnectionError(String),
    /// API request failed.
    #[deprecated(
        since = "0.1.0",
        note = "out of scope for persistence-only store; will be removed"
    )]
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

    /// A stored string value does not match any known variant.
    #[error("Unrecognized {type_name} value in DB: {value}")]
    UnrecognizedValue {
        /// The domain type that was being parsed.
        type_name: &'static str,
        /// The raw string value from the database.
        value: String,
    },

    /// Session export uses an unsupported `format_version`.
    #[error("Unsupported session export format version: {0}")]
    UnsupportedFormatVersion(u32),

    /// Failed to (de)serialize a session export payload.
    #[error("Session export serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    /// File backup / restore failed (#239).
    #[error("Database backup error: {0}")]
    BackupError(String),

    /// `PRAGMA integrity_check` reported corruption (#239).
    #[error("Database integrity check failed: {0}")]
    IntegrityCheckFailed(String),

    /// The on-disk schema is newer than this binary can handle (#239).
    ///
    /// Recovery: upgrade the binary, or restore a backup created by a
    /// compatible version (`ene store restore <backup>`).
    #[error(
        "Database schema is newer than this binary (applied migrations not known to the binary: {unknown}). Upgrade ene, or restore a compatible backup."
    )]
    SchemaTooNew {
        /// Migration names present in the DB but not in this binary.
        unknown: String,
    },

    /// A migration failed and the pre-migration backup was restored (#239).
    #[error("Migration failed; database restored from backup {backup}: {cause}")]
    MigrationRolledBack {
        /// Path of the backup that was restored.
        backup: String,
        /// Underlying migration error.
        cause: String,
    },

    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}

/// Type alias for internal module usages.
#[deprecated(since = "0.1.0", note = "use `EneMemoryError` directly")]
pub type MemoryError = EneMemoryError;
