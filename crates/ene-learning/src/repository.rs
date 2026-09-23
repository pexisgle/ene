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
    InferenceUnavailable { reason: String },
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

#[derive(Clone, PartialEq, Eq)]
pub struct MemoryChange {
    pub target: MemoryTarget,
    pub scope: LearningScope,
    pub content: String,
    pub importance: Importance,
    pub temporal: TemporalMeaning,
    pub change: ChangeKind,
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
