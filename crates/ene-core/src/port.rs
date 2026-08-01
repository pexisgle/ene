//! `MemoryPort` — the trait boundary between `ene-mind`'s cognitive logic
//! and whichever concrete store backs it.
//!
//! Defined in `ene-core` (rather than `ene-mind` or `ene-store`) so that both
//! crates can depend on it without introducing a cycle: `ene-store` depends
//! on `ene-core` to `impl MemoryPort for MemoryStore`, and `ene-mind`
//! depends on `ene-core` to call `&dyn MemoryPort` instead of the concrete
//! `ene_store::MemoryStore`. Placing the trait in either `ene-store` or
//! `ene-mind` directly would force the other to depend on it, recreating the
//! very layering violation this port exists to prevent.
//!
//! The method set here is intentionally narrow — it is not a full mirror of
//! `MemoryStore`'s API surface.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::affect::{AffectState, PendingAffectProposal};
use crate::commitment::{Commitment, NewCommitment};
use crate::memory::{
    GatheredCandidate, MemoryItem, MemoryKind, MemoryStatus, NewMemoryItem, Query,
};
use crate::pending::{NaturalDecayReport, PendingCandidate};
use crate::pending_write::PendingMemoryWrite;
use crate::span::{ActiveSceneSummaryRow, NewMemorySpan};

/// Error type for [`MemoryPort`] operations.
///
/// Store implementations convert their own persistence errors into this
/// type; callers in `ene-mind` never see a concrete backend error type.
#[derive(Debug, Error)]
pub enum MemoryPortError {
    /// The backing store rejected or failed an operation.
    #[error("memory port backend error: {0}")]
    Backend(String),

    /// An embedding vector failed structural validation (wrong length, NaN, etc.).
    #[error("invalid embedding: {0}")]
    InvalidEmbedding(String),

    /// A memory lifecycle status transition is not permitted.
    #[error("invalid memory status transition: {from:?} -> {to:?}")]
    InvalidTransition {
        /// Current status.
        from: MemoryStatus,
        /// Requested target status.
        to: MemoryStatus,
    },

    /// Catch-all for backend errors that don't fit the variants above.
    #[error("memory port error: {0}")]
    Other(String),
}

/// Abstraction over the store operations `ene-mind`'s cognitive logic needs.
///
/// Implemented for `ene_store::MemoryStore`. A lightweight in-memory test
/// double also implements this trait so cognitive logic (recall scoring,
/// arbiter decisions, forgetting) can be unit-tested without `SQLite`.
#[async_trait]
pub trait MemoryPort: Send + Sync {
    /// Insert a new typed memory item and return its assigned ID.
    async fn insert_typed_memory(&self, item: &NewMemoryItem) -> Result<i64, MemoryPortError>;

    /// List typed memories for a character, optionally filtered by kind.
    async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<MemoryKind>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError>;

    /// List active typed memories whose `source_ref` starts with `prefix`.
    async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError>;

    /// Returns the active typed memory for `character_id` + `source_ref`, if any.
    async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<MemoryItem>, MemoryPortError>;

    /// Archive active typed memories under `prefixes` whose `source_ref` is not kept.
    async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &HashSet<String>,
    ) -> Result<usize, MemoryPortError>;

    /// Gather typed-memory candidates with explainable hybrid scoring.
    ///
    /// Returns *gathered* candidates (vector/lexical/commitment/recent sources
    /// merged by memory id) **without** applying the weighted scoring policy —
    /// scoring is the `ene-rag` layer's job. Callers compose: the store gathers,
    /// `ene-rag` scores and ranks.
    async fn search(&self, query: &Query<'_>) -> Result<Vec<GatheredCandidate>, MemoryPortError>;

    /// Atomically insert a replacement memory and mark the prior row superseded.
    async fn supersede_typed_memory(
        &self,
        new_item: &NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryPortError>;

    /// Bump the access count and last-accessed timestamp for a typed memory.
    async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryPortError>;

    /// Transition a typed memory with lifecycle edge validation.
    async fn set_memory_status(
        &self,
        id: i64,
        new_status: MemoryStatus,
    ) -> Result<bool, MemoryPortError>;

    /// Apply natural decay transitions for recallable memories in a scope.
    async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        half_life_days: f64,
        fade_threshold: f32,
        archive_threshold: f32,
    ) -> Result<NaturalDecayReport, MemoryPortError>;

    /// Store a content embedding for a typed memory item.
    async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryPortError>;

    /// Insert a candidate into the user-confirmation queue.
    ///
    /// Async because the reference implementation persists the queue to the
    /// `pending_candidates` table so candidates survive restarts and are
    /// shared across `MemoryStore` instances on the same database.
    async fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError>;

    /// Enforce the pending-candidate retention policy for a character.
    ///
    /// Deletes candidates older than `max_age_days` (across all statuses) and
    /// trims the live `pending` queue down to `max_per_character` rows,
    /// dropping the oldest overflow. Both passes are scoped to `character_id`
    /// and, when `user_id` is `Some`, to that user's rows plus character-shared
    /// rows (`user_id = ""`) — mirroring the visibility rule used by natural
    /// decay — so one user's candidates cannot evict another's on a multi-user
    /// database. A `max_age_days` of `0` disables age expiry; a
    /// `max_per_character` of `0` disables the count cap. Returns the number
    /// of rows removed.
    async fn prune_pending_candidates(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        max_age_days: u32,
        max_per_character: usize,
        now: DateTime<Utc>,
    ) -> Result<usize, MemoryPortError>;

    /// List active commitments for prompt injection (independent of vector recall).
    async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError>;

    /// Load affect state for a character.
    async fn get_affect_state(&self, character_id: &str) -> Result<AffectState, MemoryPortError>;

    /// Persist affect state.
    async fn upsert_affect_state(&self, affect: &AffectState) -> Result<(), MemoryPortError>;

    /// Fetch and consume a pending post-turn classifier proposal.
    async fn take_pending_affect_proposal(
        &self,
        character_id: &str,
        user_name: &str,
    ) -> Result<Option<PendingAffectProposal>, MemoryPortError>;

    /// Insert a new commitment ledger row and return its assigned ID.
    async fn insert_commitment(&self, new: &NewCommitment) -> Result<i64, MemoryPortError>;

    /// Supersede an active commitment's description and due label.
    async fn supersede_commitment(
        &self,
        id: i64,
        description: &str,
        due_label: Option<&str>,
    ) -> Result<bool, MemoryPortError>;

    /// Mark a commitment as done.
    async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryPortError>;

    /// Mark a commitment as cancelled.
    async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryPortError>;

    /// Mark overdue active commitments as stale; returns the count updated.
    async fn mark_stale_commitments(&self, now: DateTime<Utc>) -> Result<usize, MemoryPortError>;

    /// Enqueue a failed memory write for later retry.
    async fn enqueue_pending_memory_write(
        &self,
        character_id: &str,
        user_id: &str,
        payload: String,
        error_message: String,
    ) -> Result<i64, MemoryPortError>;

    /// Take up to `limit` due pending memory writes.
    async fn take_due_pending_memory_writes(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingMemoryWrite>, MemoryPortError>;

    /// Mark a pending write as completed (deletes the row).
    async fn complete_pending_memory_write(&self, id: i64) -> Result<(), MemoryPortError>;

    /// Record another failed attempt; may transition to permanent.
    async fn fail_pending_memory_write(
        &self,
        id: i64,
        error_message: String,
    ) -> Result<PendingMemoryWrite, MemoryPortError>;

    /// Insert a new compressed memory span.
    async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryPortError>;

    /// Get the active scene summary for a session.
    async fn get_active_scene_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<ActiveSceneSummaryRow>, MemoryPortError>;

    /// List memory spans for a session at a specific compression level.
    async fn list_memory_spans_by_session_and_level(
        &self,
        session_id: &str,
        level: i32,
    ) -> Result<Vec<NewMemorySpan>, MemoryPortError>;
}

/// A stored tool-embedding field row.
///
/// Plain data vocabulary for the tool-embedding index so the RAG layer
/// (`ene-rag`) can read cached embeddings through [`EmbeddingStorePort`]
/// without depending on the concrete store crate.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEmbeddingFieldRow {
    /// Namespaced tool name (e.g. `utility.question`).
    pub tool_name: String,
    /// Embedding field label (`summary`, `description`, `capability`, `example`, `negative`).
    pub field: String,
    /// Disambiguator for repeated fields (e.g. `ex_0`, `ex_1`).
    pub field_key: String,
    /// Content-derived version hash for cache invalidation.
    pub version_hash: String,
    /// Embedding model name that produced this vector.
    pub model_name: String,
    /// The embedding vector.
    pub embedding: Vec<f32>,
    /// Source text that was embedded.
    pub source_text: String,
}

/// Error type for [`EmbeddingStorePort`] operations.
#[derive(Debug, Error)]
pub enum EmbeddingStorePortError {
    /// The backing store rejected or failed an operation.
    #[error("embedding store backend error: {0}")]
    Backend(String),
}

/// Persistence abstraction for the tool-embedding index.
///
/// Without this port, `ene-rag` would call store-specific methods
/// (`list_tool_embedding_hashes`, `list_tool_embedding_fields`,
/// `upsert_tool_embedding_field`) on the concrete `MemoryStore`. The port
/// lets `ene-rag` depend only on `ene-core`, keeping the "no
/// `ene-rag → ene-store` edge" guarantee that makes a dependency cycle
/// compile-time impossible.
#[async_trait]
pub trait EmbeddingStorePort: Send + Sync {
    /// Returns `(tool_name, field, field_key, version_hash, model_name)` for
    /// every cached tool embedding row, without fetching vectors or source text.
    async fn list_tool_embedding_hashes(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, EmbeddingStorePortError>;

    /// Lists all stored tool embeddings with full data (vector + source text).
    async fn list_tool_embedding_fields(
        &self,
    ) -> Result<Vec<ToolEmbeddingFieldRow>, EmbeddingStorePortError>;

    /// Inserts or updates one field's embedding for a tool.
    async fn upsert_tool_embedding_field(
        &self,
        tool_name: &str,
        field: &str,
        field_key: &str,
        version_hash: &str,
        model_name: &str,
        embedding: &[f32],
        source_text: &str,
    ) -> Result<(), EmbeddingStorePortError>;
}

/// Error type for [`ToolFailureSignalPort`] operations.
#[derive(Debug, Error)]
pub enum ToolFailureSignalPortError {
    /// The backing store rejected or failed an operation.
    #[error("tool failure signal backend error: {0}")]
    Backend(String),
}

/// Read-only access to recent tool-failure signals for tool selection.
///
/// Tool-grounding memories (`tool failure:{tool}`) record that a tool failed
/// for a character. The tool-selection RAG pipeline (`ene-rag`) reads these
/// signals through this port so it can down-weight tools that recently failed,
/// learning "this tool failed for this use" without `ene-rag` depending on
/// `ene-mind` or `ene-store`. The dependency points the safe way: `ene-rag`
/// depends on `ene-core` (this trait), and `ene-store` implements it — the
/// same arrangement as [`EmbeddingStorePort`], which keeps a cycle
/// compile-time impossible.
#[async_trait]
pub trait ToolFailureSignalPort: Send + Sync {
    /// Returns the namespaced tool names that have a recent, active
    /// `tool failure:{tool}` reflection memory for `character_id`.
    ///
    /// "Recent" and "active" are defined by the implementor (typically:
    /// `Reflection` memories whose title starts with `tool failure:` and whose
    /// status is recallable). Failures are returned without ordering or count
    /// guarantees beyond "recent"; the caller decides how strongly to react.
    async fn recent_tool_failures(
        &self,
        character_id: &str,
    ) -> Result<Vec<String>, ToolFailureSignalPortError>;
}
