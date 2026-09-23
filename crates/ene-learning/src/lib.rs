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
