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
//! - **Tool DB IPC server**: Per-tool database access over Unix sockets / named pipes
//!   (`db_server` module), enforcing schema declarations and prefix-based table isolation
//!
//! ## Crate Boundaries
//!
//! Enforced by [AGENTS.md §4.1](../../AGENTS.md) and
//! [API v1](../../docs/reference/architecture/api-v1.md):
//!
//! - `ene-store` is the **sole owner** of the `SQLite` / `sea-orm` connection and schema
//!   for the entire workspace. No other crate (`ene-mind`, `ene-runtime`, tool
//!   binaries) opens its own database connection or issues raw SQL against
//!   `memory.db`; they call into `MemoryStore` (or, for tool binaries, the IPC-based
//!   `ene-tool-db` client backed by `ene-store`'s `db_server`) instead.
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
    reason = "MemoryError type alias is deprecated internally; callers should use EneMemoryError"
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
/// Tool-permission audit log domain model (#177).
pub mod audit;
/// File-level SQLite backup, restore, and integrity helpers (#239).
pub mod backup;
/// Companion commitment ledger domain model.
pub mod commitment;
/// Store configuration types.
pub mod config;
/// Per-tool DB IPC server (Unix socket / named pipe).
#[cfg(any(unix, windows))]
pub mod db_server;
/// `SeaORM` entities representation.
pub mod entities;
/// Memory-related error types.
pub mod error;
/// Versioned session export format (#176).
pub mod export;
/// Memory forgetting lifecycle (decay score and status transitions).
pub(crate) mod forgetting;
/// `SeaORM` schema migrations.
pub mod migrator;
/// Hybrid memory search scoring.
pub mod search;
/// Session metadata domain model (#176).
pub mod session;
/// Core memory store (`SQLite` + sqlite-vec).
pub mod store;
/// Typed memory domain model.
pub mod typed_memory;

/// Affect state types.
pub use affect::{AffectState, DiscreteEmotion, PendingAffectProposal};
/// Audit log types (#177).
pub use audit::{AuditDecision, AuditEntry, NewAuditEntry, redact_arguments};
/// Backup / integrity open options (#239).
pub use backup::{OpenOptions, list_backups, restore_database};
/// Commitment ledger types.
pub use commitment::{ActiveCommitmentPrompt, Commitment, CommitmentStatus, NewCommitment};
/// Store feature toggle configuration.
pub use config::StoreConfig;
/// Pending deferred memory-write queue (#240).
pub use entities::pending_memory_writes::{PendingMemoryWrite, PendingMemoryWriteStatus};
/// Memory error type.
pub use error::EneMemoryError;
pub use error::MemoryError;
/// Session export format types (#176).
pub use export::{
    ExportedMessage, ExportedToolLog, SESSION_EXPORT_FORMAT_VERSION, SessionExport, redact_secrets,
};
/// Forgetting lifecycle helpers.
pub use forgetting::InvalidTransition;
/// Document-to-document lexical similarity for recall diversification.
pub use search::document_lexical_similarity;
/// Session metadata types (#176).
pub use session::{NewSessionMeta, SessionMeta};
/// Core memory types.
pub use store::{
    ActiveSceneSummaryRow, ConversationLogEntry, KeyFact, MemoryStore, NaturalDecayReport,
    NewMemorySpan, PendingCandidate, PendingCandidateStatus,
};
/// Typed memory domain types.
pub use typed_memory::{
    AffectAnnotation, HybridSearchWeights, MemoryCandidateSource, MemoryConfidence, MemoryItem,
    MemoryJournalListOptions, MemoryKind, MemorySalience, MemoryScope, MemoryScoreBreakdown,
    MemorySearchOptions, MemorySource, MemoryStatus, NewMemoryItem, Query, ScoredMemory, TimeRange,
};
