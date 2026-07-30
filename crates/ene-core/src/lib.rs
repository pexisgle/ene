//! # ene-core
//!
//! Persistence-agnostic domain vocabulary for the ene AI companion (#270).
//!
//! ## Why this crate exists
//!
//! Before this crate existed, core cognitive concepts — the PAD affect
//! state, typed-memory kinds/statuses, the commitment ledger's vocabulary —
//! were defined inside `ene-store`, the `SQLite` persistence crate. That
//! inverted the intended dependency direction: `ene-mind` (documented as a
//! "Pure Cognitive Mind" in `docs/architecture.md`) had to depend on the
//! concrete persistence crate just to name its own domain concepts.
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
//! ## What lives here
//!
//! - [`AffectState`] / [`DiscreteEmotion`] / [`PendingAffectProposal`] — PAD
//!   affect state (moved from `ene-store::affect`).
//! - Typed-memory vocabulary ([`MemoryKind`], [`MemoryItem`],
//!   [`NewMemoryItem`], [`MemoryStatus`], [`MemoryScope`], [`MemorySource`],
//!   [`MemoryConfidence`], [`MemorySalience`], [`Query`], [`ScoredMemory`],
//!   [`MemoryScoreBreakdown`], etc. — moved from `ene-store::typed_memory`).
//! - Commitment ledger vocabulary ([`CommitmentStatus`], [`Commitment`],
//!   [`NewCommitment`], [`ActiveCommitmentPrompt`] — moved from
//!   `ene-store::commitment`).
//! - [`KeyFact`] — user key/value summarization output.
//! - [`PendingCandidate`] / [`PendingCandidateStatus`] / [`NaturalDecayReport`]
//!   — memory-arbiter workflow value types.
//! - [`MemoryPort`] / [`MemoryPortError`] — the trait abstraction that lets
//!   `ene-mind`'s cognitive logic run against any store implementation
//!   (SQLite-backed, in-memory test double, or otherwise) without depending
//!   on `ene_store::MemoryStore` directly.
//!
//! `SeaORM` entities, SQL, and DB-row conversions are NOT here — those stay
//! in `ene-store`, which re-exports the types above unchanged rather than
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
    EmbeddingStorePort, EmbeddingStorePortError, MemoryPort, MemoryPortError, ToolEmbeddingFieldRow,
};
pub use span::{ActiveSceneSummaryRow, NewMemorySpan};
