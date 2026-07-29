use super::types::SessionId;
use ene_core::KeyFact;

/// Reasons for a session split or compression boundary.
#[derive(Debug, Clone)]
pub enum SplitReason {
    /// Split due to inactivity timeout.
    Timeout {
        /// Minutes elapsed since the last message.
        elapsed_minutes: u64,
    },
    /// Split due to topic change detection.
    TopicChange {
        /// Cosine similarity between consecutive user message embeddings.
        similarity: f32,
    },
    /// Split due to context length pressure.
    ContextPressure {
        /// Proportion of history used (0.0–1.0).
        context_ratio: f32,
    },
    /// Split due to a high composite score across multiple factors.
    Composite {
        /// The computed split score (0.0–1.0+).
        score: f32,
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
            Self::TopicChange { similarity } => {
                write!(
                    f,
                    "Session split due to topic change (similarity: {similarity:.2})"
                )
            }
            Self::ContextPressure { context_ratio } => {
                write!(
                    f,
                    "Session split due to context pressure ({:.0}% full)",
                    context_ratio * 100.0
                )
            }
            Self::Composite { score } => {
                write!(f, "Session split with composite score {score:.2}")
            }
            Self::Manual => write!(f, "Session split manually"),
        }
    }
}

/// The result of a session split or compression operation.
#[derive(Debug, Clone)]
pub struct SplitResult {
    /// The reason the split was triggered.
    pub reason: SplitReason,
    /// The generated conversation summary.
    pub summary: String,
    /// Extracted key facts from the conversation.
    pub key_facts: Vec<KeyFact>,
    /// The ID of the new session (unchanged for compression).
    pub new_session_id: SessionId,
    /// Number of conversation-history entries in the snapshot at split time.
    pub snapshot_len: usize,
}

/// Generates a unique session identifier.
#[must_use]
pub fn generate_session_id() -> SessionId {
    SessionId::from(format!("session_{}", uuid::Uuid::new_v4()))
}
