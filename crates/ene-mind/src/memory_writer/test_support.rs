//! Lightweight in-memory [`MemoryPort`] test double (#270).
//!
//! Exists so cognitive-logic tests (arbiter decisions, recall scoring,
//! forgetting) can run against `&dyn MemoryPort` without touching `SQLite` —
//! concretely demonstrating that `ene-mind`'s cognitive logic no longer
//! needs `ene_store::MemoryStore` to be exercised. It only implements
//! enough behavior to drive the arbiter/recall code paths under test; it is
//! not a general-purpose store (e.g. `search` always returns empty, since
//! none of the arbiter tests that use this double exercise recall).

#![cfg(test)]

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ene_core::{
    Commitment, MemoryItem, MemoryKind, MemoryPort, MemoryPortError, MemoryStatus,
    NaturalDecayReport, NewMemoryItem, PendingCandidate, Query, ScoredMemory,
};
use parking_lot::Mutex;

/// In-memory [`MemoryPort`] implementation for unit tests.
#[derive(Default)]
pub struct InMemoryMemoryPort {
    items: Mutex<Vec<MemoryItem>>,
    pending: Mutex<Vec<PendingCandidate>>,
    next_id: AtomicI64,
}

impl InMemoryMemoryPort {
    /// Create an empty in-memory port.
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed the double with an already-"persisted" memory item (assigns an id).
    pub fn seed(&self, mut item: MemoryItem) -> i64 {
        let id = self.alloc_id();
        item.id = Some(id);
        self.items.lock().push(item);
        id
    }

    /// Snapshot of every item currently held, regardless of status.
    pub fn all_items(&self) -> Vec<MemoryItem> {
        self.items.lock().clone()
    }

    /// Snapshot of the pending user-confirmation queue.
    pub fn pending_candidates(&self) -> Vec<PendingCandidate> {
        self.pending.lock().clone()
    }

    fn alloc_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn materialize(id: i64, item: &NewMemoryItem) -> MemoryItem {
        let now = Utc::now();
        MemoryItem {
            id: Some(id),
            scope: item.scope,
            character_id: item.character_id.clone(),
            user_id: item.user_id.clone(),
            kind: item.kind,
            title: item.title.clone(),
            content: item.content.clone(),
            source: item.source,
            source_ref: item.source_ref.clone(),
            confidence: item.confidence,
            salience: item.salience,
            affect: item.affect,
            relationship_impact: item.relationship_impact,
            access_count: 0,
            last_accessed_at: None,
            created_at: item.created_at.unwrap_or(now),
            updated_at: now,
            valid_from: item.valid_from,
            valid_until: item.valid_until,
            status: item.status,
            supersedes_id: item.supersedes_id,
            pinned: item.pinned,
            faded_at: None,
            commitment_id: item.commitment_id,
        }
    }
}

#[async_trait]
impl MemoryPort for InMemoryMemoryPort {
    async fn insert_typed_memory(&self, item: &NewMemoryItem) -> Result<i64, MemoryPortError> {
        let id = self.alloc_id();
        self.items.lock().push(Self::materialize(id, item));
        Ok(id)
    }

    async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<MemoryKind>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        let items = self.items.lock();
        Ok(items
            .iter()
            .filter(|m| m.character_id == character_id && kind.is_none_or(|k| m.kind == k))
            .skip(offset)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        let items = self.items.lock();
        Ok(items
            .iter()
            .filter(|m| {
                m.character_id == character_id
                    && m.status == MemoryStatus::Active
                    && m.source_ref
                        .as_deref()
                        .is_some_and(|r| r.starts_with(prefix))
            })
            .take(limit)
            .cloned()
            .collect())
    }

    async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<MemoryItem>, MemoryPortError> {
        let items = self.items.lock();
        Ok(items
            .iter()
            .find(|m| {
                m.character_id == character_id
                    && m.status == MemoryStatus::Active
                    && m.source_ref.as_deref() == Some(source_ref)
            })
            .cloned())
    }

    async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &HashSet<String>,
    ) -> Result<usize, MemoryPortError> {
        let mut items = self.items.lock();
        let mut archived = 0usize;
        for item in items.iter_mut() {
            if item.character_id != character_id || item.status != MemoryStatus::Active {
                continue;
            }
            let Some(source_ref) = item.source_ref.as_deref() else {
                continue;
            };
            let matches_prefix = prefixes.iter().any(|p| source_ref.starts_with(p));
            if matches_prefix && !keep_refs.contains(source_ref) {
                item.status = MemoryStatus::Archived;
                archived += 1;
            }
        }
        Ok(archived)
    }

    async fn search(&self, _query: &Query<'_>) -> Result<Vec<ScoredMemory>, MemoryPortError> {
        // No test using this double exercises hybrid recall scoring; kept
        // trivially empty rather than reimplementing `ene-store`'s search.
        Ok(Vec::new())
    }

    async fn supersede_typed_memory(
        &self,
        new_item: &NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryPortError> {
        let new_id = self.alloc_id();
        let mut items = self.items.lock();
        if let Some(old) = items.iter_mut().find(|m| m.id == Some(superseded_id)) {
            old.status = MemoryStatus::Superseded;
            old.supersedes_id = None;
        }
        let mut inserted = Self::materialize(new_id, new_item);
        inserted.supersedes_id = Some(superseded_id);
        items.push(inserted);
        Ok(new_id)
    }

    async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryPortError> {
        let mut items = self.items.lock();
        if let Some(item) = items.iter_mut().find(|m| m.id == Some(id)) {
            item.access_count += 1;
            item.last_accessed_at = Some(Utc::now());
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn set_memory_status(
        &self,
        id: i64,
        new_status: MemoryStatus,
    ) -> Result<bool, MemoryPortError> {
        let mut items = self.items.lock();
        if let Some(item) = items.iter_mut().find(|m| m.id == Some(id)) {
            item.status = new_status;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn apply_natural_decay_batch(
        &self,
        _character_id: &str,
        _user_id: Option<&str>,
        _now: DateTime<Utc>,
        _half_life_days: f64,
        _limit: usize,
    ) -> Result<NaturalDecayReport, MemoryPortError> {
        Ok(NaturalDecayReport::default())
    }

    async fn upsert_memory_embedding(
        &self,
        _memory_item_id: i64,
        _model_name: &str,
        _field: &str,
        _embedding: &[f32],
    ) -> Result<(), MemoryPortError> {
        Ok(())
    }

    fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError> {
        let id = self.alloc_id();
        let mut stored = candidate;
        stored.id = id;
        self.pending.lock().push(stored);
        Ok(id)
    }

    async fn list_active_commitments(
        &self,
        _character_id: &str,
        _user_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        Ok(Vec::new())
    }
}
