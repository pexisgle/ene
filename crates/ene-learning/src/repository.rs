//! The durable boundary for Memory, Summary, and revision persistence.
//!
//! The implementation commits one change and its evidence in one short atomic
//! section: the expected revision is compared against the current row inside
//! that section, so a stale formation result can never overwrite a newer
//! recognition. Stale and missing outcomes are domain outcomes on the `Ok`
//! side, never technical errors.

use ene_credential::CredentialSetRevision;
use ene_primitive::{RawId, WallClockWithTz};
use thiserror::Error;

use crate::identity::{MemoryId, MemoryRevision, SummaryId};
use crate::memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
use crate::scope::LearningScope;
use crate::summary::SummaryRecord;

/// Infrastructure failure for Learning persistence and formation.
///
/// Stale / missing / scope outcomes are [`MemoryChangeOutcome`], and an
/// uninterpretable model answer is
/// [`FormationDecision::DeferredForContext`](crate::FormationDecision), never
/// this error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LearningTechnicalError {
    #[error("learning storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause. Never Memory or Summary content.
        reason: String,
    },
    /// A commit reused a Summary identity with a different payload.
    ///
    /// Summary identity names one piece of evidence. Several changes of one
    /// formation deliberately share the same record, but a caller that offers
    /// the same id with different content, scope, source, or formation time is
    /// asking to silently rebind that evidence. Nothing is written; a changed
    /// payload needs a fresh identity.
    #[error("summary identity conflicts with the stored evidence")]
    SummaryIdentityConflict {
        /// The reused identity; carries no content.
        summary: SummaryId,
    },
    #[error("learning inference unavailable: {reason}")]
    InferenceUnavailable {
        /// Provider-class cause. Never prompt or output text.
        reason: String,
    },
    /// The secret boundary could not prove registered values absent.
    ///
    /// The text was neither sent nor stored; the caller may retry once the
    /// registry and stored values are readable.
    #[error("secret boundary unavailable: {reason}")]
    SecretBoundaryUnavailable {
        /// Boundary-class cause. Never prompt or output text.
        reason: String,
    },
}

/// Which Memory one change targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTarget {
    /// A Memory formed for the first time. The id is minted by the caller so
    /// the committed identity is known before the commit.
    New { id: MemoryId },
    /// An existing Memory that must still be at `expected_revision`.
    Existing {
        id: MemoryId,
        expected_revision: MemoryRevision,
    },
}

/// One prospective Memory change: the content, its meaning, and the change
/// kind relative to the target's previous revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryChange {
    pub target: MemoryTarget,
    /// The scope the change is made under. For an existing Memory the stored
    /// scope must agree; a change never widens or moves a Memory.
    pub scope: LearningScope,
    /// Proposed recognition text.
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub change: ChangeKind,
    /// `true` only for a normal-forgetting suppression.
    pub recall_suppressed: bool,
    /// When this change was decided; becomes the revision and current-row
    /// timestamp.
    pub at: WallClockWithTz,
}

/// One atomic commit: evidence Summary (when present) plus one Memory change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryChangeCommit {
    /// Inserted once when first supplied and reused verbatim by later changes
    /// of the same formation; carries no authority beyond evidence.
    pub summary: Option<SummaryRecord>,
    /// Credential-set premise the committed content was scrubbed under.
    ///
    /// The implementation compares it against the current durable set inside
    /// the commit transaction; [`MemoryChangeOutcome::StaleCredentialSet`]
    /// refuses content scrubbed before a credential became registered.
    /// [`None`] skips the check (tests and non-content commits).
    pub secret_premise: Option<CredentialSetRevision>,
    pub change: MemoryChange,
}

/// Outcome of one Memory change commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryChangeOutcome {
    Committed {
        memory: MemoryId,
        revision: MemoryRevision,
    },
    /// The stored revision did not match `expected_revision`; nothing was
    /// written and the newer recognition is left untouched.
    StaleTarget {
        memory: MemoryId,
        current: MemoryRevision,
    },
    /// The target Memory does not exist.
    MissingTarget { memory: MemoryId },
    /// The target exists under a different scope; a change never moves it.
    ScopeMismatch { memory: MemoryId },
    /// A new Memory id was already used. Identity is never reused.
    AlreadyExists { memory: MemoryId },
    /// The target ran out of distinct revisions; nothing was written.
    RevisionExhausted { memory: MemoryId },
    /// The credential set moved past `secret_premise`; nothing was written.
    ///
    /// The caller must not retry the same content: it may carry the newly
    /// registered value and needs a fresh scrub and currentness premise.
    StaleCredentialSet,
}

/// Durable Learning boundary.
///
/// The implementation keeps the compare of [`MemoryTarget::Existing`] and the
/// durable update in one atomic section, and never holds a transaction across
/// caller I/O. Reads return the stored state as-is; currentness is decided by
/// the caller from the returned revision.
#[expect(
    async_fn_in_trait,
    reason = "Stage 2 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait LearningRepository: Send + Sync {
    /// Commits one Memory change and its evidence atomically.
    async fn commit_memory_change(
        &self,
        commit: MemoryChangeCommit,
    ) -> Result<MemoryChangeOutcome, LearningTechnicalError>;

    /// Lists current memories for one Companion, most recently formed first,
    /// capped at `limit`. Suppressed memories are included: suppression is a
    /// recall decision, not a visibility restriction.
    ///
    /// `after` starts the page strictly older than that Memory, so a caller
    /// can page through the whole set by passing the last id it received.
    /// `None` starts at the newest.
    async fn list_current_memories(
        &self,
        companion: RawId,
        after: Option<MemoryId>,
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError>;

    /// Lists a bounded page of one Memory's revisions, oldest first,
    /// including the initial one.
    ///
    /// `after` starts strictly after that revision, so every revision stays
    /// reachable by feeding the last returned one back. `limit` bounds the
    /// rows read; zero requests none.
    async fn list_memory_revisions(
        &self,
        memory: MemoryId,
        after: Option<MemoryRevision>,
        limit: u64,
    ) -> Result<Vec<MemoryRevisionRecord>, LearningTechnicalError>;

    /// Retrieves bounded recall candidates for one companion.
    ///
    /// Returns current, non-suppressed memories that are among the newest
    /// `limit` rows, among the most important `limit` rows, or match one of
    /// `terms`, with each arm capped by `limit`. Suppression is excluded
    /// before the caps apply, so suppressed rows never consume candidate
    /// slots. The caller ranks the returned candidates; the retrieval
    /// carries no persisted score and does not decide canonical importance.
    async fn recall_candidates(
        &self,
        companion: RawId,
        terms: &[String],
        limit: u64,
    ) -> Result<Vec<Memory>, LearningTechnicalError>;

    /// Loads the Summaries named by `ids` in one bounded batch.
    ///
    /// Duplicate ids are read once, and ids with no stored Summary are
    /// absent from the result: a missing identity is never fabricated, and
    /// the caller can render it as its own state.
    async fn load_summaries(
        &self,
        ids: &[SummaryId],
    ) -> Result<Vec<SummaryRecord>, LearningTechnicalError>;
}
