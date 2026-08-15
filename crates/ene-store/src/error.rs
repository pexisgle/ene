use thiserror::Error;

/// Error types for the memory subsystem.
///
/// Marked `#[non_exhaustive]` so downstream crates (notably `ene-runtime`'s
/// `public_api::PublicApiError` boundary) cannot exhaustively match
/// this enum without a wildcard arm. That means adding a new variant here
/// never breaks a downstream crate's compile — new variants silently fall
/// through any wildcard arm until that crate chooses to special-case them.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum EneMemoryError {
    /// `SeaORM` error from the memory store.
    #[error("Memory store error: {0}")]
    MemoryStoreError(#[from] sea_orm::DbErr),
    #[error("Memory store connection error: {0}")]
    MemoryStoreConnectionError(String),
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

    #[error("Invalid memory status transition: {from:?} -> {to:?}")]
    InvalidTransition {
        from: crate::MemoryStatus,
        to: crate::MemoryStatus,
    },

    #[error("Unrecognized {type_name} value in DB: {value}")]
    UnrecognizedValue {
        type_name: &'static str,
        value: String,
    },

    #[error("Invalid pending candidate edit: {0}")]
    InvalidPendingCandidateEdit(String),

    #[error("Invalid memory edit: {0}")]
    InvalidMemoryEdit(String),

    #[error("Unsupported session export format version: {0}")]
    UnsupportedFormatVersion(u32),

    #[error("Session export serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Invalid schedule: {0}")]
    InvalidSchedule(#[from] ene_core::ScheduleError),

    #[error("Database backup error: {0}")]
    BackupError(String),

    /// `PRAGMA integrity_check` reported corruption.
    #[error("Database integrity check failed: {0}")]
    IntegrityCheckFailed(String),

    /// The on-disk schema is newer than this binary can handle.
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

    #[error("Migration failed; database restored from backup {backup}: {cause}")]
    MigrationRolledBack { backup: String, cause: String },

    /// Catch-all error variant.
    #[error("Other error: {0}")]
    Other(String),
}
