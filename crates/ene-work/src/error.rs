use thiserror::Error;

/// Work-plane failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WorkError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(String),
    #[error("unknown job {0}")]
    UnknownJob(String),
    #[error("unknown schedule {0}")]
    UnknownSchedule(String),
    #[error("unknown skill {0}")]
    UnknownSkill(String),
    #[error("unknown delegation {0}")]
    UnknownDelegation(String),
    #[error("schedule: {0}")]
    Schedule(String),
    #[error("already completed")]
    AlreadyCompleted,
    #[error("cancelled")]
    Cancelled,
    #[error("skill: {0}")]
    Skill(String),
    #[error("no free job slot")]
    SlotsFull,
    #[error("delegation depth exceeded")]
    DepthExceeded,
    #[error("internal delegation cannot spawn a public child")]
    SecrecyViolation,
    #[error("unsupported artifact kind {0}")]
    UnsupportedArtifact(String),
}
