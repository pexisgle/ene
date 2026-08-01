use super::types::SessionId;
use ene_core::KeyFact;

/// Reasons for starting a **new session** (#369).
///
/// Topic changes and context pressure are handled by compression (#368), not
/// session splits. [`SplitReason::Composite`] from the old design was removed;
/// topic-boundary detection exposes only a score via [`TopicBoundarySignal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitReason {
    /// Split due to inactivity timeout.
    Timeout {
        /// Minutes elapsed since the last message.
        elapsed_minutes: u64,
    },
    /// Split requested manually by the user.
    Manual,
}

impl std::fmt::Display for SplitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { elapsed_minutes } => {
                write!(
                    f,
                    "Session split due to {elapsed_minutes} minutes of inactivity"
                )
            }
            Self::Manual => write!(f, "Session split manually"),
        }
    }
}

/// Outcome of a **session split** that issues a new [`SessionId`] (#369).
///
/// Compression-only operations return [`crate::context::CompressionResult`]
/// instead; they do not change the session id.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// Why the split was triggered.
    pub reason: SplitReason,
    /// The generated conversation summary (compression artifact).
    pub summary: String,
    /// Extracted key facts from the conversation.
    pub key_facts: Vec<KeyFact>,
    /// The session id after the operation. Unchanged for compression-only passes.
    pub new_session_id: SessionId,
    /// Number of conversation-history entries in the snapshot at split time.
    pub snapshot_len: usize,
}

/// Generates a unique session identifier.
#[must_use]
pub fn generate_session_id() -> SessionId {
    SessionId::from(format!("session_{}", uuid::Uuid::new_v4()))
}
