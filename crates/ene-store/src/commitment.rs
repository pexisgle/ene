use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing;

/// Lifecycle status of a companion commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    /// Open and should be surfaced in prompts / recall.
    Active,
    /// User or companion marked the commitment fulfilled.
    Done,
    /// Explicitly cancelled or withdrawn.
    Cancelled,
    /// No longer actionable (expired or superseded without completion).
    Stale,
}

impl CommitmentStatus {
    /// Returns the `snake_case` string representation for storage/DB use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Stale => "stale",
        }
    }

    /// Parse from a `snake_case` string, logging a warning on unrecognized values.
    pub(crate) fn from_db_str(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            "stale" => Self::Stale,
            other => {
                tracing::warn!(
                    other,
                    "unrecognized CommitmentStatus in DB, falling back to Active"
                );
                Self::Active
            }
        }
    }
}

/// A tracked companion promise, task, or follow-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commitment {
    /// Primary key (`None` until persisted).
    pub id: Option<i64>,
    /// Character identifier.
    pub character_id: String,
    /// User identifier (may be empty).
    pub user_id: String,
    /// Short label for the commitment.
    pub title: String,
    /// Full description / context.
    pub description: String,
    /// Lifecycle status.
    pub status: CommitmentStatus,
    /// Parsed due datetime, if known.
    pub due_at: Option<DateTime<Utc>>,
    /// Raw due hint from extraction (e.g. "tomorrow", "次回").
    pub due_label: Option<String>,
    /// Linked typed memory row (`MemoryKind::Commitment`).
    pub source_memory_id: Option<i64>,
    /// When the commitment was created.
    pub created_at: DateTime<Utc>,
    /// When the commitment was last updated.
    pub updated_at: DateTime<Utc>,
    /// When the commitment was completed (`Done` only).
    pub completed_at: Option<DateTime<Utc>>,
}

/// Payload for creating a new commitment row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCommitment {
    /// Character identifier.
    pub character_id: String,
    /// User identifier (may be empty).
    pub user_id: String,
    /// Short label.
    pub title: String,
    /// Full description.
    pub description: String,
    /// Initial status (typically `Active`).
    pub status: CommitmentStatus,
    /// Parsed due datetime, if known.
    pub due_at: Option<DateTime<Utc>>,
    /// Raw due hint from extraction.
    pub due_label: Option<String>,
    /// Linked typed memory row.
    pub source_memory_id: Option<i64>,
}

/// Lightweight DTO for prompt injection (independent of vector recall).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveCommitmentPrompt {
    /// Commitment id.
    pub id: i64,
    /// Short label.
    pub title: String,
    /// Full description.
    pub description: String,
    /// Raw due hint, if any.
    pub due_label: Option<String>,
    /// Parsed due datetime, if any.
    pub due_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commitment_status_serde_roundtrip() {
        for status in [
            CommitmentStatus::Active,
            CommitmentStatus::Done,
            CommitmentStatus::Cancelled,
            CommitmentStatus::Stale,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: CommitmentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(status, back);
        }
    }
}
