use thiserror::Error;

/// Companion-layer failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CompanionError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(String),
    #[error("unknown soul {0}")]
    UnknownSoul(String),
    #[error("unknown memory {0}")]
    UnknownMemory(String),
    #[error("memory candidate is no longer pending: {0}")]
    CandidateConflict(String),
    #[error("invalid memory candidate: {0}")]
    InvalidCandidate(String),
    #[error("package: {0}")]
    Package(String),
    #[error("format_version {found} is not supported (expected {expected})")]
    UnknownFormat { found: u32, expected: u32 },
    #[error("package digest mismatch")]
    DigestMismatch,
    #[error("package exceeds size limit ({0} bytes)")]
    PackageTooLarge(u64),
    #[error("classify: {0}")]
    Classify(String),
    #[error("invalid id {0}")]
    InvalidId(String),
}

impl CompanionError {
    #[must_use]
    pub fn package(msg: impl Into<String>) -> Self {
        Self::Package(msg.into())
    }

    #[must_use]
    pub fn codec(msg: impl Into<String>) -> Self {
        Self::Codec(msg.into())
    }
}
