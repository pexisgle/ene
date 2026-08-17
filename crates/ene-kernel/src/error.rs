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
