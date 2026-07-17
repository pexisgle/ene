//! # ene-store
//!
//! SQLite-vec powered episodic memory store for the ene AI character platform.
//!
//! ## Features
//!
//! - **Conversation summaries**: Persistent storage of conversation summaries with vector embeddings
//! - **Key facts**: User-specific key-value facts with upsert and latest-value retrieval
//! - **Vector similarity search**: Cosine-similarity-based recall of semantically relevant past conversations
//! - **Tool RAG**: Embedding-based tool selection (stored in `tool_embedding_index` table, multi-vector per tool)
//! - **Conversation logging**: Full conversation history in `conversation_logs` for audit and replay
//!
//! ## Crate Boundaries
//!
//! Enforced by [AGENTS.md §4.1](../../AGENTS.md) and
//! [API v2](../../docs/architecture/api-v2.md):
//!
//! - `ene-store` is the **sole owner** of the `SQLite` / `sea-orm` connection and schema
//!   for the entire workspace. No other crate (`ene-mind`, `ene-runtime`, tool
//!   binaries) opens its own database connection or issues raw SQL against
//!   `memory.db`; they call into `MemoryStore` (or, for tool binaries, the IPC-based
//!   `ene-tool-db` client backed by `ene-runtime`'s `db_server`) instead.
//! - Depends on: `ene-config`. The store has no LLM, embedding
//!   provider, or prompt-assembly dependency; callers supply vectors and the mind
//!   runtime owns summarization and prompt formatting.
//!   It does NOT depend on `ene-runtime`, `ene-ai`, `ene-mind`, or
//!   `ene-tool-proto` — the store sits low in the dependency graph so it can be
//!   safely called from any of those crates without introducing a cycle.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use ene_store::MemoryStore;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let store = MemoryStore::open(std::path::Path::new(":memory:"), 768).await?;
//!     // Typed-memory search is the primary API — see `typed_memory::Query`.
//!     Ok(())
//! }
//! ```
#![warn(missing_docs)]
#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred locally"
)]
#![expect(
    deprecated,
    reason = "sea-orm migration API still uses deprecated items"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::clone_on_copy,
        reason = "unit/integration tests use unwrap/expect for assertions",
    )
)]

/// Affect state domain model.
pub mod affect;
/// Companion commitment ledger domain model.
pub mod commitment;
/// Store configuration types.
pub mod config;
/// `SeaORM` entities representation.
pub mod entities;
/// Memory-related error types.
pub mod error;
/// Memory forgetting lifecycle (decay score and status transitions).
pub mod forgetting;
/// Legacy memory table migration (#98).
pub mod legacy_migration;
/// `SeaORM` schema migrations.
pub mod migrator;
/// Hybrid memory search scoring.
pub mod search;
/// Core memory store (`SQLite` + sqlite-vec).
pub mod store;
/// Typed memory domain model.
pub mod typed_memory;

/// Affect state types.
pub use affect::{AffectState, DiscreteEmotion, PendingAffectProposal};
/// Commitment ledger types.
pub use commitment::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
/// Store feature toggle configuration.
pub use config::StoreConfig;
/// Memory error type.
pub use error::EneMemoryError;
pub use error::MemoryError;
/// Forgetting lifecycle helpers.
pub use forgetting::{
    ARCHIVE_THRESHOLD, FADE_THRESHOLD, InvalidTransition, active_decay_anchor, decay_score,
    emotional_impact, faded_decay_anchor, target_status_after_decay, user_restorable_statuses,
    validate_transition, validate_user_restore,
};
/// Legacy migration types and orchestration (#98).
pub use legacy_migration::{
    LegacyMigrationOptions, LegacyMigrationReport, LegacyRowCounts, MigrationStatus,
    execute_legacy_migration, keyfact_kind_for_key,
};
/// Document-to-document lexical similarity for recall diversification.
pub use search::document_lexical_similarity;
/// Core memory types.
pub use store::{
    ActiveSceneSummaryRow, ConversationSummary, KeyFact, LegacyWriteMode, MemoryStore,
    NaturalDecayReport, NewMemorySpan, RecalledSummary,
};
/// Typed memory domain types.
pub use typed_memory::{
    AffectAnnotation, HybridSearchWeights, MemoryCandidateSource, MemoryConfidence, MemoryItem,
    MemoryJournalListOptions, MemoryKind, MemorySalience, MemoryScope, MemoryScoreBreakdown,
    MemorySearchOptions, MemorySource, MemoryStatus, NewMemoryItem, Query, ScoredMemory,
};
