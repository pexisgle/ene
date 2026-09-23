//! Learning ownership: Experience Summary evidence, Memory current
//! recognition, and their revisions and grounds.
//!
//! A [`Memory`] is the current recognition a Companion uses later; a
//! [`SummaryRecord`] is the compressed evidence one formation or update judged
//! from, never a second current-knowledge store. A [`MemoryRevisionRecord`]
//! keeps the change history so a correction can be distinguished from a
//! situation that changed, and normal forgetting never deletes content.
//!
//! This crate owns the semantics and the [`LearningRepository`] contract; the
//! persistent implementation lives behind that trait (see `ene-store`), and
//! inference is supplied as an opaque port by the caller, so this crate takes
//! no dependency on history, task, permission, or inference crates. Its
//! `ene-credential` dependency is limited to the non-secret scrub boundary
//! types re-exported below; credential values and credential state remain
//! outside this crate. Cross-domain identities arrive as `RawId` premises and
//! are never converted into another domain's newtype.
//!
//! Stage 3 implements the Companion scope only. Global scope is deliberately
//! absent: a companion-derived memory cannot be widened by accident, and the
//! scope field keeps the distinction explicit for the later stage.

mod formation;
mod identity;
mod memory;
mod recall;
mod relevance;
mod repository;
mod scope;
mod summary;

#[doc(no_inline)]
pub use ene_credential::{CredentialSetRevision, ScrubbedText, SecretScrubError, SecretScrubber};
pub use formation::{
    ExperienceCandidate, ExperienceRole, ExperienceTurn, FormationDecision, LearningInference,
    LearningInferenceAnswer, LearningInferenceError, LearningInferencePremise,
    MAX_FORMATION_CHANGES, MAX_FORMATION_TURNS, form_experience,
};
pub use identity::{
    ExperienceSourceKind, LearningClaimRef, MemoryId, MemoryRevision, SourceRangeRef, SummaryId,
};
pub use memory::{ChangeKind, Importance, Memory, MemoryRevisionRecord, TemporalMeaning};
pub use recall::{RECALL_CANDIDATE_LIMIT, RecallQuery, RecalledMemory, recall};
pub use relevance::recall_index_terms;
pub use repository::{
    LearningRepository, LearningTechnicalError, MemoryChange, MemoryChangeCommit,
    MemoryChangeOutcome, MemoryTarget,
};
pub use scope::LearningScope;
pub use summary::SummaryRecord;
