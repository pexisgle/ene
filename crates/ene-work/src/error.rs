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
    #[error("unknown execution {0}")]
    UnknownExecution(String),
    #[error("no open question")]
    NoOpenQuestion,
    #[error("question already resolved")]
    QuestionAlreadyResolved,
    #[error("expected {expected} answers, got {actual}")]
    QuestionAnswerCount { expected: usize, actual: usize },
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
    #[error("mutating work needs an approved plan")]
    PlanNotApproved,
    #[error("job lane: {0}")]
    JobLane(String),
    #[error("invalid contract: {0}")]
    InvalidContract(String),
    #[error("scope widening pending approval: {tools:?}")]
    ScopeWideningPending { tools: Vec<String> },
    #[error("no scope widening pending")]
    NoPendingScopeWidening,
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("workspace violation: {0}")]
    WorkspaceViolation(String),
    #[error("interrupted")]
    Interrupted,
    #[error("unsupported artifact kind {0}")]
    UnsupportedArtifact(String),
}
