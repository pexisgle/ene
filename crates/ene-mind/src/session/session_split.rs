use super::types::SessionId;
use ene_core::KeyFact;

/// Reasons for starting a **new session**.
///
/// Topic changes and context pressure are handled by compression, not session
/// splits. Topic-boundary detection exposes only a score via
/// [`TopicBoundarySignal`] and never triggers a split directly.
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

/// Outcome of a **session split** that issues a new [`SessionId`].
///
/// Compression-only operations return [`crate::context::CompressionResult`]
/// instead; they do not change the session id. [`handle_manual_compression`] in
/// the runtime still returns this type for API compatibility, but
/// [`Self::new_session_id`] is unchanged for compression-only passes.
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
