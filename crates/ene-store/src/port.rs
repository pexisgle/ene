//! `impl MemoryPort for MemoryStore`.
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
    ActiveSceneSummaryRow, AffectState, Commitment, EmbeddingStorePort, EmbeddingStorePortError,
    GatheredCandidate, MemoryItem, MemoryKind, MemoryPort, MemoryPortError, MemoryStatus,
    NaturalDecayReport, NewCommitment, NewMemoryItem, NewMemorySpan, NewWorkspaceChunk,
    PendingAffectProposal, PendingCandidate, PendingMemoryWrite, Query, ToolEmbeddingFieldRow,
    ToolFailureSignalPort, ToolFailureSignalPortError, WorkspaceChunkHit, WorkspaceDocumentPort,
    WorkspaceFileRow, WorkspaceIndexStatus, WorkspacePortError, WorkspaceSearchQuery,
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

impl From<EneMemoryError> for WorkspacePortError {
    fn from(err: EneMemoryError) -> Self {
        Self::Backend(err.to_string())
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
        user_id: Option<&str>,
        status: Option<MemoryStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        Ok(Self::get_typed_memories_by_character(
            self,
            character_id,
            kind,
            user_id,
            status,
            limit,
            offset,
        )
        .await?)
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

    async fn record_memory_outcome(
        &self,
        outcome: &ene_core::MemoryOutcome,
    ) -> Result<i64, MemoryPortError> {
        Ok(Self::record_memory_outcome(self, outcome).await?)
    }

    async fn list_memory_outcomes(
        &self,
        character_id: &str,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<Vec<ene_core::MemoryOutcome>, MemoryPortError> {
        Ok(Self::list_memory_outcomes(self, character_id, since, limit).await?)
    }

    async fn delete_memory_outcomes(
        &self,
        character_id: &str,
        ids: &[i64],
    ) -> Result<usize, MemoryPortError> {
        Ok(Self::delete_memory_outcomes(self, character_id, ids).await?)
    }

    async fn search(&self, query: &Query<'_>) -> Result<Vec<GatheredCandidate>, MemoryPortError> {
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
        fade_threshold: f32,
        archive_threshold: f32,
    ) -> Result<NaturalDecayReport, MemoryPortError> {
        Ok(Self::apply_natural_decay_batch(
            self,
            character_id,
            user_id,
            now,
            half_life_days,
            fade_threshold,
            archive_threshold,
        )
        .await?)
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

    async fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError> {
        Ok(Self::insert_pending_candidate(self, candidate).await?)
    }

    async fn prune_pending_candidates(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        max_age_days: u32,
        max_per_character: usize,
        now: DateTime<Utc>,
    ) -> Result<usize, MemoryPortError> {
        Ok(Self::prune_pending_candidates(
            self,
            character_id,
            user_id,
            max_age_days,
            max_per_character,
            now,
        )
        .await?)
    }

    async fn list_pending_candidates(
        &self,
        character_id: &str,
        status_filter: Option<ene_core::PendingCandidateStatus>,
    ) -> Result<Vec<PendingCandidate>, MemoryPortError> {
        Ok(Self::list_pending_candidates(self, character_id, status_filter).await?)
    }

    async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        Ok(Self::list_active_commitments(self, character_id, user_id, limit).await?)
    }

    async fn get_affect_state(&self, character_id: &str) -> Result<AffectState, MemoryPortError> {
        Ok(Self::get_affect_state(self, character_id).await?)
    }

    async fn upsert_affect_state(&self, affect: &AffectState) -> Result<(), MemoryPortError> {
        Ok(Self::upsert_affect_state(self, affect).await?)
    }

    async fn take_pending_affect_proposal(
        &self,
        character_id: &str,
        user_name: &str,
    ) -> Result<Option<PendingAffectProposal>, MemoryPortError> {
        Ok(Self::take_pending_affect_proposal(self, character_id, user_name).await?)
    }

    async fn insert_commitment(&self, new: &NewCommitment) -> Result<i64, MemoryPortError> {
        Ok(Self::insert_commitment(self, new).await?)
    }

    async fn supersede_commitment(
        &self,
        id: i64,
        description: &str,
        due_label: Option<&str>,
    ) -> Result<bool, MemoryPortError> {
        Ok(Self::supersede_commitment(self, id, description, due_label).await?)
    }

    async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryPortError> {
        Ok(Self::complete_commitment(self, id).await?)
    }

    async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryPortError> {
        Ok(Self::cancel_commitment(self, id).await?)
    }

    async fn mark_stale_commitments(&self, now: DateTime<Utc>) -> Result<usize, MemoryPortError> {
        Ok(Self::mark_stale_commitments(self, now).await?)
    }

    async fn enqueue_pending_memory_write(
        &self,
        character_id: &str,
        user_id: &str,
        payload: String,
        error_message: String,
    ) -> Result<i64, MemoryPortError> {
        Ok(
            Self::enqueue_pending_memory_write(self, character_id, user_id, payload, error_message)
                .await?,
        )
    }

    async fn take_due_pending_memory_writes(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingMemoryWrite>, MemoryPortError> {
        Ok(Self::take_due_pending_memory_writes(self, limit).await?)
    }

    async fn complete_pending_memory_write(&self, id: i64) -> Result<(), MemoryPortError> {
        Ok(Self::complete_pending_memory_write(self, id).await?)
    }

    async fn fail_pending_memory_write(
        &self,
        id: i64,
        error_message: String,
    ) -> Result<PendingMemoryWrite, MemoryPortError> {
        Ok(Self::fail_pending_memory_write(self, id, error_message).await?)
    }

    async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryPortError> {
        Ok(Self::insert_memory_span(self, span).await?)
    }

    async fn get_active_scene_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<ActiveSceneSummaryRow>, MemoryPortError> {
        Ok(Self::get_active_scene_summary(self, session_id).await?)
    }

    async fn list_memory_spans_by_session_and_level(
        &self,
        session_id: &str,
        level: i32,
    ) -> Result<Vec<NewMemorySpan>, MemoryPortError> {
        Ok(Self::list_memory_spans_by_session_and_level(self, session_id, level).await?)
    }
}

#[async_trait]
impl WorkspaceDocumentPort for MemoryStore {
    async fn list_workspace_files(&self) -> Result<Vec<WorkspaceFileRow>, WorkspacePortError> {
        Ok(Self::list_workspace_files(self).await?)
    }

    async fn replace_workspace_file(
        &self,
        file: &WorkspaceFileRow,
        chunks: &[NewWorkspaceChunk],
    ) -> Result<(), WorkspacePortError> {
        Ok(Self::replace_workspace_file(self, file, chunks).await?)
    }

    async fn rename_workspace_file(
        &self,
        old_path: &str,
        file: &WorkspaceFileRow,
    ) -> Result<bool, WorkspacePortError> {
        Ok(Self::rename_workspace_file(self, old_path, file).await?)
    }

    async fn delete_workspace_files(&self, paths: &[String]) -> Result<usize, WorkspacePortError> {
        Ok(Self::delete_workspace_files(self, paths).await?)
    }

    async fn prune_workspace_roots(
        &self,
        keep_roots: &[String],
    ) -> Result<usize, WorkspacePortError> {
        Ok(Self::prune_workspace_roots(self, keep_roots).await?)
    }

    async fn search_workspace(
        &self,
        query: &WorkspaceSearchQuery<'_>,
    ) -> Result<Vec<WorkspaceChunkHit>, WorkspacePortError> {
        Ok(Self::search_workspace(self, query).await?)
    }

    async fn workspace_index_status(&self) -> Result<WorkspaceIndexStatus, WorkspacePortError> {
        Ok(Self::workspace_index_status(self).await?)
    }
}

impl From<EneMemoryError> for EmbeddingStorePortError {
    fn from(err: EneMemoryError) -> Self {
        Self::Backend(err.to_string())
    }
}

#[async_trait]
impl EmbeddingStorePort for MemoryStore {
    async fn list_tool_embedding_hashes(
        &self,
    ) -> Result<Vec<(String, String, String, String, String)>, EmbeddingStorePortError> {
        Ok(Self::list_tool_embedding_hashes(self).await?)
    }

    async fn list_tool_embedding_fields(
        &self,
    ) -> Result<Vec<ToolEmbeddingFieldRow>, EmbeddingStorePortError> {
        Ok(Self::list_tool_embedding_fields(self).await?)
    }

    async fn upsert_tool_embedding_field(
        &self,
        tool_name: &str,
        field: &str,
        field_key: &str,
        version_hash: &str,
        model_name: &str,
        embedding: &[f32],
        source_text: &str,
    ) -> Result<(), EmbeddingStorePortError> {
        Ok(Self::upsert_tool_embedding_field(
            self,
            tool_name,
            field,
            field_key,
            version_hash,
            model_name,
            embedding,
            source_text,
        )
        .await?)
    }
}

impl From<EneMemoryError> for ToolFailureSignalPortError {
    fn from(err: EneMemoryError) -> Self {
        Self::Backend(err.to_string())
    }
}

#[async_trait]
impl ToolFailureSignalPort for MemoryStore {
    async fn recent_tool_failures(
        &self,
        character_id: &str,
    ) -> Result<Vec<String>, ToolFailureSignalPortError> {
        Ok(Self::recent_tool_failures(self, character_id).await?)
    }
}
