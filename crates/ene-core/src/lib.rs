//! # ene-core
//!
//! Persistence-agnostic domain vocabulary for the ene AI companion.
//!
//! ## Why this crate exists
//!
//! Core cognitive concepts — the PAD affect state, typed-memory
//! kinds/statuses, the commitment ledger's vocabulary — live here rather
//! than in the `SQLite` persistence crate, so that `ene-mind` (documented
//! as a "Pure Cognitive Mind" in `docs/architecture.md`) does not have to
//! depend on the concrete persistence crate just to name its own domain
//! concepts.
//!
//! `ene-core` sits below both:
//!
//! ```text
//! ene-core  ←  ene-store   (SeaORM entities, SQL, backup/migration)
//! ene-core  ←  ene-mind    (cognitive logic: recall, arbiter, forgetting, ...)
//! ```
//!
//! It depends on nothing internal to the workspace — only `serde`, `chrono`,
//! `thiserror`, `tracing`, `schemars`, and `async-trait` (the last two are
//! needed for [`HybridSearchWeights`]'s `JsonSchema` derive and the
//! [`MemoryPort`] trait respectively; see each module for details).
//!
//! `SeaORM` entities, SQL, and DB-row conversions are NOT here — those stay
//! in `ene-store`, which re-exports the crate's types unchanged rather than
//! redefining them.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "unit tests use unwrap/expect for assertions",
    )
)]

mod affect;
mod commitment;
mod key_fact;
mod memory;
mod pending;
mod pending_write;
mod port;
mod span;

pub use affect::{AffectState, DiscreteEmotion, PendingAffectProposal};
pub use commitment::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
pub use key_fact::KeyFact;
pub use memory::{
    AffectAnnotation, GatheredCandidate, HybridSearchWeights, MemoryCandidateSource,
    MemoryConfidence, MemoryItem, MemoryJournalListOptions, MemoryKind, MemorySalience,
    MemoryScope, MemoryScoreBreakdown, MemorySearchOptions, MemorySource, MemoryStatus,
    NewMemoryItem, Query, ScoredMemory, TimeRange,
};
pub use pending::{NaturalDecayReport, PendingCandidate, PendingCandidateStatus};
pub use pending_write::{PendingMemoryWrite, PendingMemoryWriteStatus};
pub use port::{
    EmbeddingStorePort, EmbeddingStorePortError, MemoryPort, MemoryPortError,
    ToolEmbeddingFieldRow, ToolFailureSignalPort, ToolFailureSignalPortError,
};
pub use span::{ActiveSceneSummaryRow, NewMemorySpan};
