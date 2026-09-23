use ene_credential::CredentialSetRevision;
use ene_primitive::{RawId, WallClockWithTz};
use thiserror::Error;

use crate::identity::{LearningClaimRef, MemoryId, MemoryRevision, SummaryId};
use crate::memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
use crate::scope::LearningScope;
use crate::summary::SummaryRecord;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LearningTechnicalError {
    #[error("learning storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
    #[error("summary identity conflicts with the stored evidence")]
    SummaryIdentityConflict { summary: SummaryId },
    #[error("learning inference unavailable: {reason}")]
    InferenceUnavailable {
        /// Provider-class cause. Never prompt or output text.
        reason: String,
    },
    /// The secret boundary could not prove registered values absent, or the
    /// credential set moved past the scrub premise.
    ///
    /// The unproven text was neither sent nor stored. The stale-credential-set
    /// refusal can follow changes of the same pass that already committed, so
    /// the caller must not retry the same content: it may carry the newly
    /// registered value and needs a fresh scrub and currentness premise.
    #[error("secret boundary unavailable: {reason}")]
    SecretBoundaryUnavailable { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTarget {
    New {
        id: MemoryId,
    },
    Existing {
        id: MemoryId,
        expected_revision: MemoryRevision,
    },
}

/// One prospective Memory change: the content, its meaning, and the change
/// kind relative to the target's previous revision.
#[derive(Clone, PartialEq, Eq)]
pub struct MemoryChange {
    pub target: MemoryTarget,
    pub scope: LearningScope,
    /// Proposed recognition text; redacted from `core::fmt::Debug` because it
    /// may quote owner speech or secret-bearing material before scrubbing.
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub change: ChangeKind,
    /// When this change was decided; becomes the revision and current-row
    /// timestamp.
    pub at: WallClockWithTz,
}

impl core::fmt::Debug for MemoryChange {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("MemoryChange")
            .field("target", &self.target)
            .field("scope", &self.scope)
            .field("content", &"[redacted]")
            .field("importance", &self.importance)
            .field("temporal", &self.temporal)
            .field("change", &self.change)
            .field("at", &self.at)
            .finish()
    }
}

/// One atomic commit: evidence Summary (when present) plus one Memory change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryChangeCommit {
    pub summary: Option<SummaryRecord>,
    pub secret_premise: Option<CredentialSetRevision>,
    pub claim: Option<LearningClaimRef>,
    pub change: MemoryChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryChangeOutcome {
    Committed {
        memory: MemoryId,
        revision: MemoryRevision,
    },
    StaleTarget {
        memory: MemoryId,
        current: MemoryRevision,
    },
    MissingTarget {
        memory: MemoryId,
    },
    ScopeMismatch {
        memory: MemoryId,
    },
    AlreadyExists {
        memory: MemoryId,
    },
    RevisionExhausted {
        memory: MemoryId,
    },
    StaleCredentialSet,
    HeldForErasure,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait LearningRepository: Send + Sync {
    async fn commit_memory_change(
        &self,
        commit: MemoryChangeCommit,
    ) -> Result<MemoryChangeOutcome, LearningTechnicalError>;

    async fn list_current_memories(
        &self,
        companion: RawId,
        after: Option<MemoryId>,
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError>;

    async fn list_memory_revisions(
        &self,
        memory: MemoryId,
        after: Option<MemoryRevision>,
        limit: u64,
    ) -> Result<Vec<MemoryRevisionRecord>, LearningTechnicalError>;

    /// Retrieves bounded recall candidates for one companion.
    ///
    /// Returns current, non-suppressed memories that are among the newest
    /// `limit` rows, among the most important `limit` rows, or carry one of
    /// `terms` in the derived token index, with each arm capped by `limit`.
    /// Candidates are returned newest-first with duplicate ids removed, so
    /// the caller's stable ranking keeps recency as its final tie-break.
    /// Lexical matching is token equality against
    /// [`recall_index_terms`](crate::recall_index_terms): a query term
    /// matches a Memory whose content derives that same token, so an old
    /// relevant Memory stays reachable without scanning stored content.
    /// Suppression is excluded before the caps apply, so suppressed rows
    /// never consume candidate slots. Every arm is served by an index, so
    /// growing unrelated rows does not turn candidate lookup into a full
    /// scan or sort. The caller ranks the returned candidates; the retrieval
    /// carries no persisted score and does not decide canonical importance.
    async fn recall_candidates(
        &self,
        companion: RawId,
        terms: &[String],
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError>;

    async fn load_summaries(
        &self,
        ids: &[SummaryId],
    ) -> Result<Vec<SummaryRecord>, LearningTechnicalError>;
}
