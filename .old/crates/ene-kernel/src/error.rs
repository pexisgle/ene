use ene_session::{LaneId, TurnId};
use thiserror::Error;

/// Kernel / lane-command failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KernelError {
    #[error(transparent)]
    Session(#[from] ene_session::SessionError),
    #[error("lane busy (turn {turn_id})")]
    LaneBusy { turn_id: TurnId },
    #[error("no active operation on lane {lane}")]
    NoActiveOperation { lane: String },
    #[error("invalid message: {0}")]
    InvalidMessage(String),
    #[error("lane is closed")]
    Closed,
    #[error("kernel is shutting down")]
    ShuttingDown,
    #[error("nothing to compact")]
    NothingToCompact,
    #[error("model: {0}")]
    Model(String),
    #[error("queued entry not found")]
    QueuedNotFound,
    #[error("tool: {0}")]
    Tool(String),
}

impl KernelError {
    #[must_use]
    pub fn lane_busy(turn_id: TurnId) -> Self {
        Self::LaneBusy { turn_id }
    }

    #[must_use]
    pub fn no_active(lane: &LaneId) -> Self {
        Self::NoActiveOperation {
            lane: lane.as_str(),
        }
    }

    /// Wire `error_class` for HTTP problem+json (lane-api §5).
    #[must_use]
    pub fn error_class(&self) -> &'static str {
        match self {
            Self::LaneBusy { .. } => "lane_busy",
            Self::NoActiveOperation { .. } => "no_active_operation",
            Self::InvalidMessage(_) => "invalid_message",
            Self::Closed | Self::ShuttingDown => "closed",
            Self::NothingToCompact => "nothing_to_compact",
            Self::QueuedNotFound => "not_found",
            Self::Session(_) => "fault",
            Self::Model(_) | Self::Tool(_) => "failed",
        }
    }
}

/// Three-way result of `cancel_queued` (I-27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelQueued {
    Cancelled,
    AlreadyConsumed,
    NotFound,
}

/// Outcome of an accepted `prompt` / turn (command still returns `Ok`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}
