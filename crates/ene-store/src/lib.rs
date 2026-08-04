//! # ene-store
//!
//! SQLite-vec powered episodic memory store for the ene AI character platform.
//!
//! ## Features
//!
//! - **Key facts**: User-specific key-value facts (domain type for summarization output)
//! - **Vector similarity search**: Cosine-similarity-based recall of semantically relevant memories
//! - **Tool RAG**: Embedding-based tool selection (stored in `tool_embedding_index` table, multi-vector per tool)
//! - **Conversation logging**: Full conversation history in `conversation_logs` for audit and replay
//! - **Host service (`db`)**: Shared host-service socket with a `db` passenger
//!   (`host_service` + `db_server`), enforcing schema declarations and
//!   prefix-based table isolation
//!
//! ## Crate Boundaries
//!
//! Enforced by the [API v1](../../docs/reference/architecture/api-v1.md) architecture
//! boundaries:
//!
//! - `ene-store` is the **sole owner** of the `SQLite` / `sea-orm` connection and schema
//!   for the entire workspace. No other crate (`ene-mind`, `ene-runtime`, tool
//!   binaries) opens its own database connection or issues raw SQL against
//!   `memory.db`; they call into `MemoryStore` (or, for plugin binaries, the IPC-based
//!   `ene-plugin-db` client backed by `ene-store`'s host-service `db`
//!   passenger) instead.
//! - Depends on: `ene-config`, `ene-core`. The store has no LLM, embedding
//!   provider, or prompt-assembly dependency; callers supply vectors and the mind
//!   runtime owns summarization and prompt formatting.
//!   It does NOT depend on `ene-runtime`, `ene-ai`, `ene-mind`, or
//!   `ene-tool-proto` — the store sits low in the dependency graph so it can be
//!   safely called from any of those crates without introducing a cycle.
//! - Domain vocabulary (`AffectState`, typed-memory kinds/statuses, the
//!   commitment ledger's types) is defined in `ene-core` and
//!   re-exported here unchanged — `ene-store` owns only the `SeaORM`
//!   entities and SQL that convert those domain types to/from DB rows. It
//!   additionally implements `ene_core::MemoryPort` for `MemoryStore`, the
//!   trait `ene-mind`'s cognitive logic programs against instead of this
//!   concrete type.
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
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "unit/integration tests use unwrap/expect/panic/indexing for assertions",
    )
)]

/// Affect state domain model.
pub mod affect;
/// Tool-permission audit log domain model.
pub mod audit;
/// File-level SQLite backup, restore, and integrity helpers.
pub mod backup;
/// Companion commitment ledger domain model.
pub mod commitment;
/// Store configuration types.
pub mod config;
/// Per-tool DB IPC request handler (also used standalone in tests).
#[cfg(any(unix, windows))]
pub mod db_server;
/// `SeaORM` entities representation.
pub mod entities;
/// Memory-related error types.
pub mod error;
/// Versioned session export format.
pub mod export;
/// Memory forgetting lifecycle (decay score and status transitions).
pub(crate) mod forgetting;
/// Multiplexed host-service acceptor (`db` passenger today).
#[cfg(any(unix, windows))]
pub mod host_service;
/// `SeaORM` schema migrations.
pub mod migrator;
/// `impl MemoryPort for MemoryStore`.
pub mod port;
/// Hybrid memory search scoring.
pub mod search;
/// Persistent scheduler domain model.
pub mod schedule;
/// Session metadata domain model.
pub mod session;
/// Core memory store (`SQLite` + sqlite-vec).
pub mod store;
/// Typed memory domain model.
pub mod typed_memory;

/// Affect state types.
pub use affect::{AffectState, DiscreteEmotion, PendingAffectProposal};
/// Audit log types.
pub use audit::{
    AuditDecision, AuditEntry, NewAuditEntry, redact_arguments, redact_arguments_for_tool,
};
/// Backup / integrity open options.
pub use backup::{OpenOptions, list_backups, restore_database};
/// Commitment ledger types.
pub use commitment::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
/// Store feature toggle configuration.
pub use config::StoreConfig;
/// Memory error type.
pub use error::EneMemoryError;
/// Session export format types.
pub use export::{
    ExportedMessage, ExportedToolLog, SESSION_EXPORT_FORMAT_VERSION, SessionExport, redact_secrets,
};
/// Forgetting lifecycle helpers.
pub use forgetting::InvalidTransition;
/// Document-to-document lexical similarity for recall diversification.
pub use search::document_lexical_similarity;
/// Persistent scheduler domain types.
pub use schedule::{
    NewSchedule, Schedule, ScheduleAction, ScheduleConfirmation, ScheduleError, ScheduleKind,
    ScheduleRun, ScheduleRunStatus,
};
/// Session metadata types.
pub use session::{NewSessionMeta, SessionMeta};
/// Core memory types.
pub use store::{
    ActiveSceneSummaryRow, ClaimedFire, ConversationLogEntry, FireClaimMode, KeyFact, MemoryStore,
    NaturalDecayReport, NewMemorySpan, PendingCandidate, PendingCandidateStatus,
};
/// Typed memory domain types.
pub use typed_memory::{
    AffectAnnotation, HybridSearchWeights, MemoryCandidateSource, MemoryConfidence, MemoryItem,
    MemoryJournalListOptions, MemoryKind, MemorySalience, MemoryScope, MemoryScoreBreakdown,
    MemorySearchOptions, MemorySource, MemoryStatus, NewMemoryItem, Query, ScoredMemory, TimeRange,
};
