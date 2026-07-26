//! `MemoryPort` — the trait boundary between `ene-mind`'s cognitive logic
//! and whichever concrete store backs it (#270).
//!
//! Defined in `ene-core` (rather than `ene-mind` or `ene-store`) so that both
//! crates can depend on it without introducing a cycle: `ene-store` depends
//! on `ene-core` to `impl MemoryPort for MemoryStore`, and `ene-mind`
//! depends on `ene-core` to call `&dyn MemoryPort` instead of the concrete
//! `ene_store::MemoryStore`. Placing the trait in either `ene-store` or
//! `ene-mind` directly would force the other to depend on it, recreating the
//! very layering violation this issue fixes.
//!
//! The method set here is intentionally narrow: it covers exactly the store
//! operations called by the cognitive-logic modules that were converted to
//! use this trait (`ene-mind`'s recall runner, memory arbiter, forgetting
//! lifecycle, character CCv3/style sync, memory journal, self-reflection,
//! lorebook boost, and the commitment ledger's `list_active`). It is not a
//! full mirror of `MemoryStore`'s API surface.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::commitment::Commitment;
use crate::memory::{MemoryItem, MemoryKind, MemoryStatus, NewMemoryItem, Query, ScoredMemory};
use crate::pending::{NaturalDecayReport, PendingCandidate};

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

    /// Search typed memories with explainable hybrid scoring (#123).
    async fn search(&self, query: &Query<'_>) -> Result<Vec<ScoredMemory>, MemoryPortError>;

    /// Atomically insert a replacement memory and mark the prior row superseded.
    async fn supersede_typed_memory(
        &self,
        new_item: &NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryPortError>;

    /// Bump the access count and last-accessed timestamp for a typed memory.
    async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryPortError>;

    /// Transition a typed memory with lifecycle edge validation (#76).
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
        limit: usize,
    ) -> Result<NaturalDecayReport, MemoryPortError>;

    /// Store a content embedding for a typed memory item.
    async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryPortError>;

    /// Insert a candidate into the user-confirmation queue (#174).
    ///
    /// Synchronous because the reference implementation keeps this queue
    /// in-memory (see `ene_store::MemoryStore`); implementations backed by a
    /// database may still perform synchronous/blocking I/O here if needed.
    fn insert_pending_candidate(&self, candidate: PendingCandidate)
    -> Result<i64, MemoryPortError>;

    /// List active commitments for prompt injection (independent of vector recall).
    async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError>;
}
