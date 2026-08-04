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
    /// Outcome rating of the interaction that produced this candidate
    /// (-1.0 negative ..= 1.0 positive), carried through deferral so an
    /// approved candidate still enters the self-reflection loop.
    #[serde(default)]
    pub outcome_rating: Option<f32>,
    /// Source quote from the conversation that triggered this candidate.
    pub source_quote: String,
    /// Source turn that triggered this candidate, when the extraction ran
    /// inside a turn.
    ///
    /// The runtime's `TurnId` string is stored so the approval UI can point
    /// back at the conversation that produced the candidate. `None` for
    /// candidates produced outside a turn (retried writes, tests) or rows
    /// persisted before this field existed.
    pub source_turn: Option<String>,
    /// Whether the candidate was parked by approval mode
    /// (`mind.memory_approval.require_approval`) rather than by a
    /// weak-contradiction deferral.
    ///
    /// Approval-parked rows are excluded from unconfirmed recall regardless
    /// of the current mode, so toggling approval off cannot leak candidates
    /// that were never approved. Weak-contradiction rows keep their
    /// `[unconfirmed]` recall behavior.
    pub approval_parked: bool,
    /// Workflow status.
    pub status: PendingCandidateStatus,
    /// When the candidate was created.
    ///
    /// Persisted to the `pending_candidates` table and used as the
    /// anchor for the age-based retention sweep. Callers inserting a new
    /// candidate set this to [`Utc::now`].
    pub created_at: DateTime<Utc>,
    /// When the candidate was resolved (approved or rejected).
    ///
    /// `None` while the candidate is still pending. Persisted so history
    /// views can show when the decision was made without an extra audit
    /// table.
    pub resolved_at: Option<DateTime<Utc>>,
}

/// User-editable fields of a pending memory candidate.
///
/// The source quote, extraction reason, conflict target, and provenance are
/// fixed at extraction time and deliberately not editable; only the content
/// a user would want to correct before approval is exposed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingCandidateEdit {
    /// Short title or label.
    pub title: String,
    /// Full candidate content.
    pub content: String,
    /// Memory kind.
    pub kind: MemoryKind,
    /// Confidence score (0.0 .. 1.0).
    pub confidence: f32,
}

/// Result of a natural-decay batch run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NaturalDecayReport {
    /// Memories transitioned to `faded`.
    pub faded_count: usize,
    /// Memories transitioned to `archived`.
    pub archived_count: usize,
}
