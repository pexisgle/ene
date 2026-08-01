//! Pending memory-candidate workflow vocabulary and decay-report DTO,
//! exchanged between `ene-mind`'s arbiter and [`crate::MemoryPort`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::memory::MemoryKind;

/// Workflow status of a pending memory candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingCandidateStatus {
    /// Awaiting user review.
    Pending,
    /// Approved by the user (persisted to typed memory).
    Approved,
    /// Rejected by the user.
    Rejected,
}

impl PendingCandidateStatus {
    /// Returns the `snake_case` string representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }

    /// Decode a stored status label.
    ///
    /// Returns `None` for an unrecognized label. Callers must fail closed on
    /// `None` (exclude the row) rather than defaulting to [`Self::Pending`] —
    /// a corrupted label silently resurrecting a row into the live queue
    /// would let it be approved again.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }
}

/// A pending memory candidate awaiting user approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingCandidate {
    /// Primary key.
    pub id: i64,
    /// Character identifier.
    pub character_id: String,
    /// User identifier (may be empty).
    pub user_id: String,
    /// Short title or label.
    pub title: String,
    /// Full candidate content.
    pub content: String,
    /// Memory kind.
    pub kind: MemoryKind,
    /// Confidence score (0.0 .. 1.0).
    pub confidence: f32,
    /// Human-readable reason for the extraction.
    pub reason_detail: String,
    /// Title of the existing memory this candidate would supersede, if any.
    ///
    /// A denormalized display label captured from the conflicting memory at
    /// insert time so the approval UI can render the conflict without a join.
    /// It is **not** persisted — the `pending_candidates` table stores only
    /// [`Self::existing_memory_id`] — so it does not survive a DB round-trip
    /// and is `None` on rows rehydrated from storage; presentation layers
    /// needing it then resolve the title by joining on
    /// [`Self::existing_memory_id`] at list time. `None` when the candidate
    /// does not conflict with an existing memory.
    pub existing_memory_title: Option<String>,
    /// Id of the existing typed memory this candidate would supersede, if any.
    ///
    /// Persisted alongside the candidate so the approval flow can
    /// resolve the supersede target without re-searching. `None` when the
    /// candidate does not conflict with an existing memory.
    pub existing_memory_id: Option<i64>,
    /// Source quote from the conversation that triggered this candidate.
    pub source_quote: String,
    /// Workflow status.
    pub status: PendingCandidateStatus,
    /// When the candidate was created.
    ///
    /// Persisted to the `pending_candidates` table and used as the
    /// anchor for the age-based retention sweep. Callers inserting a new
    /// candidate set this to [`Utc::now`].
    pub created_at: DateTime<Utc>,
}

/// Result of a natural-decay batch run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NaturalDecayReport {
    /// Memories transitioned to `faded`.
    pub faded_count: usize,
    /// Memories transitioned to `archived`.
    pub archived_count: usize,
}
