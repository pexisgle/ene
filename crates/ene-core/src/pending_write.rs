//! Pending deferred memory-write queue domain types consumed by `ene-mind`'s
//! deferred memory pipeline through [`crate::MemoryPort`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle status of a queued memory write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingMemoryWriteStatus {
    /// Waiting for (re)try.
    Pending,
    /// Exhausted retries; needs user attention.
    Permanent,
}

impl PendingMemoryWriteStatus {
    /// Stable string label for persistence / display.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Permanent => "permanent",
        }
    }

    /// Parse a stored status label.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "permanent" => Some(Self::Permanent),
            _ => None,
        }
    }
}

/// Domain row for a deferred memory-write retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMemoryWrite {
    /// Primary key.
    pub id: i64,
    /// Character scope.
    pub character_id: String,
    /// User scope.
    pub user_id: String,
    /// JSON-encoded payload.
    pub payload_json: String,
    /// Attempts already made (including the original failure).
    pub attempts: i32,
    /// Maximum attempts before becoming permanent.
    pub max_attempts: i32,
    /// Last error message.
    pub last_error: Option<String>,
    /// Queue status.
    pub status: PendingMemoryWriteStatus,
    /// When the row was first created.
    pub created_at: DateTime<Utc>,
    /// Earliest time a retry should run.
    pub next_retry_at: DateTime<Utc>,
    /// Last update time.
    pub updated_at: DateTime<Utc>,
}
