use thiserror::Error;

/// Session-log and usage-ledger failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    /// Underlying `SQLite` failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// `MessagePack` encode/decode failure.
    #[error("payload codec: {0}")]
    Codec(String),
    /// JSON encode/decode failure (export).
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// `PRAGMA integrity_check` reported corruption.
    #[error("database integrity check failed: {0}")]
    IntegrityCheckFailed(String),
    /// On-disk `storage_version` is newer than this binary.
    #[error(
        "storage version {found} is newer than this binary (supports {supported}); upgrade ene, or restore a compatible backup"
    )]
    StorageTooNew { found: u32, supported: u32 },
    /// Identifier was not a valid `UUIDv7` string.
    #[error("invalid id: {0}")]
    InvalidId(String),
    /// Session row is missing (parent entry absence is always corruption).
    #[error("session not found: {0}")]
    SessionNotFound(String),
    /// Event referenced a session that has no row.
    #[error("parent session row missing for event in {0}")]
    MissingParent(String),
    /// Writer actor has stopped.
    #[error("session writer is closed")]
    WriterClosed,
    /// Fork boundary is past the source session's last seq.
    #[error("fork boundary {boundary} exceeds source next_seq {next_seq}")]
    ForkBoundary { boundary: u64, next_seq: u64 },
    /// Seq gap detected at open (L-2).
    #[error("seq gap in session {session_id}: expected {expected}, found {found}")]
    SeqGap {
        session_id: String,
        expected: u64,
        found: u64,
    },
    /// Export or projection requested an unknown session.
    #[error("cannot {op} unknown session {session_id}")]
    UnknownSession {
        op: &'static str,
        session_id: String,
    },
    /// I/O failure (spill files, export paths).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Transaction mixed events from more than one session.
    #[error("transaction entries must share a single session_id")]
    MixedSessionTransaction,
    /// Arithmetic overflow while allocating seq.
    #[error("seq overflow in session {0}")]
    SeqOverflow(String),
}

impl SessionError {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "used as map_err(SessionError::codec)"
    )]
    pub(crate) fn codec(err: impl ToString) -> Self {
        Self::Codec(err.to_string())
    }
}
