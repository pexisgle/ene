//! `impl MemoryPort for MemoryStore` (#270).
//!
//! Thin delegation to the inherent methods on [`crate::store::MemoryStore`]
//! defined elsewhere in this crate; the only real work here is converting
//! [`crate::error::EneMemoryError`] into [`ene_core::MemoryPortError`] so
//! `ene-mind`'s cognitive logic never needs to name this crate's error type.
//!
//! `ene-store`'s own public API (the inherent `async fn`s on `MemoryStore`)
//! is unchanged by this — this trait impl is purely additive, for callers
//! that want to program against `&dyn MemoryPort` instead of the concrete
//! store type.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ene_core::{
    Commitment, MemoryItem, MemoryKind, MemoryPort, MemoryPortError, MemoryStatus,
    NaturalDecayReport, NewMemoryItem, PendingCandidate, Query, ScoredMemory,
};

use crate::error::EneMemoryError;
use crate::store::MemoryStore;

impl From<EneMemoryError> for MemoryPortError {
    fn from(err: EneMemoryError) -> Self {
        match err {
            EneMemoryError::InvalidEmbedding(msg) => Self::InvalidEmbedding(msg),
            EneMemoryError::InvalidTransition { from, to } => Self::InvalidTransition { from, to },
            other => Self::Backend(other.to_string()),
        }
    }
}

#[async_trait]
impl MemoryPort for MemoryStore {
    async fn insert_typed_memory(&self, item: &NewMemoryItem) -> Result<i64, MemoryPortError> {
        Ok(Self::insert_typed_memory(self, item).await?)
    }

    async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<MemoryKind>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        Ok(Self::get_typed_memories_by_character(self, character_id, kind, limit, offset).await?)
    }

    async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        Ok(Self::list_typed_memories_by_source_prefix(self, character_id, prefix, limit).await?)
    }

    async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<MemoryItem>, MemoryPortError> {
        Ok(Self::get_active_typed_memory_by_source_ref(self, character_id, source_ref).await?)
    }

    async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &HashSet<String>,
    ) -> Result<usize, MemoryPortError> {
        Ok(
            Self::archive_typed_memories_by_source_prefixes(
                self,
                character_id,
                prefixes,
                keep_refs,
            )
            .await?,
        )
    }

    async fn search(&self, query: &Query<'_>) -> Result<Vec<ScoredMemory>, MemoryPortError> {
        Ok(Self::search(self, query).await?)
    }

    async fn supersede_typed_memory(
        &self,
        new_item: &NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryPortError> {
        Ok(Self::supersede_typed_memory(self, new_item, superseded_id).await?)
    }

    async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryPortError> {
        Ok(Self::bump_typed_memory_access(self, id).await?)
    }

    async fn set_memory_status(
        &self,
        id: i64,
        new_status: MemoryStatus,
    ) -> Result<bool, MemoryPortError> {
        Ok(Self::set_memory_status(self, id, new_status).await?)
    }

    async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        half_life_days: f64,
        limit: usize,
    ) -> Result<NaturalDecayReport, MemoryPortError> {
        Ok(
            Self::apply_natural_decay_batch(
                self,
                character_id,
                user_id,
                now,
                half_life_days,
                limit,
            )
            .await?,
        )
    }

    async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryPortError> {
        Ok(
            Self::upsert_memory_embedding(self, memory_item_id, model_name, field, embedding)
                .await?,
        )
    }

    fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError> {
        Ok(Self::insert_pending_candidate(self, candidate)?)
    }

    async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        Ok(Self::list_active_commitments(self, character_id, user_id, limit).await?)
    }
}
