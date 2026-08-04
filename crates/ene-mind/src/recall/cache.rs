//! L1 in-memory tier for recall planning.
//!
//! `before_turn` issues four recurring L2 queries: active commitments (twice),
//! the hybrid gather (`MemoryPort::search`, which covers the vec0 ANN, FTS5
//! lexical matches, commitment-linked rows, and the recent fallback), the
//! pending-candidate queue, and reflection memories. This module caches those
//! results in memory so repeated turns skip `SQLite` entirely; `ene-store`
//! remains the only persistence owner and the L2 fallback on every miss.
//!
//! Correctness contract: a cached gather is only served when the backing rows
//! are provably unchanged. Every write path that `ene-mind` knows about
//! invalidates the affected character's entries (the post-turn writer pipeline
//! through [`WriteTrackingPort`], session split through
//! [`crate::session::ConversationSession::reset_session`], and runtime memory
//! mutations through the shared cache handle). Access bumps are the one
//! per-turn mutation and are applied in place via [`MemoryRecallCache::refresh_access`],
//! so cached candidates score identically to freshly gathered ones.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use blake3::Hash as Blake3Hash;
use chrono::Utc;
use ene_core::{
    Commitment, GatheredCandidate, MemoryItem, MemoryKind, MemoryPort, MemoryPortError,
    PendingCandidate, PendingCandidateStatus, Query,
};
use indexmap::IndexMap;
use parking_lot::RwLock;
use tracing::debug;

/// Upper bound on per-scope cached lists (commitments / pending / reflections).
const MAX_SCOPE_ENTRIES: usize = 16;

/// Upper bound on cached gather results (hot vector-embedding queries).
const MAX_SEARCH_ENTRIES: usize = 64;

/// Cache statistics for hit/miss telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallCacheStats {
    /// Number of L2 queries avoided.
    pub hits: u64,
    /// Number of L2 fallbacks performed.
    pub misses: u64,
    /// Number of invalidation operations applied.
    pub invalidations: u64,
}

/// L1 in-memory recall cache shared across turns.
///
/// All sections are bounded; eviction is FIFO on insertion order. Entries are
/// plain data clones, so lookups never hold the lock across an `await`.
#[derive(Debug, Default)]
pub struct MemoryRecallCache {
    inner: RwLock<Inner>,
    /// Bumped by every invalidation and access refresh; miss-path inserts are
    /// gated on it so a read that started before a mutation cannot land its
    /// pre-mutation snapshot afterwards.
    epoch: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    invalidations: AtomicU64,
}

#[derive(Debug, Default)]
struct Inner {
    commitments: IndexMap<ScopeKey, CachedCommitments>,
    pending: IndexMap<ScopeKey, Vec<PendingCandidate>>,
    reflections: IndexMap<String, Vec<MemoryItem>>,
    searches: IndexMap<SearchKey, Vec<GatheredCandidate>>,
}

impl Inner {
    fn new() -> Self {
        Self {
            commitments: IndexMap::new(),
            pending: IndexMap::new(),
            reflections: IndexMap::new(),
            searches: IndexMap::new(),
        }
    }
}

/// Scope identity for character/user-keyed sections.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ScopeKey {
    character_id: String,
    user_id: Option<String>,
}

impl ScopeKey {
    fn new(character_id: &str, user_id: Option<&str>) -> Self {
        Self {
            character_id: character_id.to_string(),
            user_id: user_id.map(ToOwned::to_owned),
        }
    }
}

/// Cached active-commitment list plus the limit it was loaded with.
///
/// The store returns rows ordered by due date, so any smaller limit can be
/// served by truncation; a larger limit requires a fresh L2 read.
#[derive(Debug, Clone)]
struct CachedCommitments {
    commitments: Vec<Commitment>,
    limit: usize,
}

/// Identity of a hybrid gather result.
///
/// Only parameters the store's gather step consumes are part of the key;
/// scoring-only fields (`weights`, `min_score`, decay half-lives, `now`,
/// `time_range`, `query_affect`) are re-applied to the cached candidates every
/// turn, so they must not fragment the cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SearchKey {
    session_id: String,
    character_id: String,
    user_id: Option<String>,
    model_name: String,
    query_text: String,
    embedding_hash: Option<Blake3Hash>,
    similarity_threshold_bits: u32,
    candidate_pool_size: usize,
    limit: usize,
    recent_fallback_limit: usize,
    exclude_kinds: Vec<MemoryKind>,
}

impl SearchKey {
    fn from_query(session_id: &str, query: &Query<'_>) -> Self {
        Self {
            session_id: session_id.to_string(),
            character_id: query.character_id.to_string(),
            user_id: query.user_id.map(ToOwned::to_owned),
            model_name: query.model_name.to_string(),
            query_text: query.query_text.to_string(),
            embedding_hash: query.embedding.map(|embedding| {
                let mut hasher = blake3::Hasher::new();
                for value in embedding {
                    hasher.update(&value.to_le_bytes());
                }
                hasher.finalize()
            }),
            similarity_threshold_bits: query.similarity_threshold.to_bits(),
            candidate_pool_size: query.candidate_pool_size,
            limit: query.limit,
            recent_fallback_limit: query.recent_fallback_limit,
            exclude_kinds: query.exclude_kinds.clone(),
        }
    }
}

impl MemoryRecallCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::new()),
            epoch: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
        }
    }

    /// Current hit/miss/invalidation counters.
    #[must_use]
    pub fn stats(&self) -> RecallCacheStats {
        RecallCacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            invalidations: self.invalidations.load(Ordering::Relaxed),
        }
    }

    /// Drop every entry belonging to `character_id`.
    ///
    /// The user dimension is deliberately ignored: a write to one user can
    /// change character-shared rows (and therefore every user's gather), so a
    /// character-wide drop is the conservative correct action.
    pub fn invalidate_character(&self, character_id: &str) {
        let mut inner = self.inner.write();
        inner
            .commitments
            .retain(|key, _| key.character_id != character_id);
        inner
            .pending
            .retain(|key, _| key.character_id != character_id);
        inner.reflections.shift_remove(character_id);
        inner
            .searches
            .retain(|key, _| key.character_id != character_id);
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.invalidations.fetch_add(1, Ordering::Relaxed);
    }

    /// Drop every entry. Called on session split and by runtime mutation
    /// handlers that only know a memory id, not its scope.
    pub fn invalidate_all(&self) {
        let mut inner = self.inner.write();
        inner.commitments.clear();
        inner.pending.clear();
        inner.reflections.clear();
        inner.searches.clear();
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.invalidations.fetch_add(1, Ordering::Relaxed);
    }

    /// Cached hybrid gather, or L2 via `store.search` on miss.
    pub async fn search(
        &self,
        store: &dyn MemoryPort,
        session_id: &str,
        query: &Query<'_>,
    ) -> Result<Vec<GatheredCandidate>, MemoryPortError> {
        let key = SearchKey::from_query(session_id, query);
        if let Some(cached) = self.inner.read().searches.get(&key) {
            self.count_hit("search");
            return Ok(cached.clone());
        }
        self.count_miss("search");
        let epoch = self.epoch.load(Ordering::Acquire);
        let gathered = store.search(query).await?;
        self.insert_if_current(epoch, |inner| {
            inner.searches.insert(key, gathered.clone());
            evict_fifo(&mut inner.searches, MAX_SEARCH_ENTRIES);
        });
        Ok(gathered)
    }

    /// Cached active commitments, or L2 on miss.
    ///
    /// A request no larger than the cached list's loaded limit is served by
    /// truncation; anything larger re-reads L2 and replaces the entry.
    pub async fn list_active_commitments(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        let key = ScopeKey::new(character_id, user_id);
        if let Some(cached) = self.inner.read().commitments.get(&key)
            && cached.limit >= limit
        {
            self.count_hit("commitments");
            return Ok(cached.commitments.iter().take(limit).cloned().collect());
        }
        self.count_miss("commitments");
        let epoch = self.epoch.load(Ordering::Acquire);
        let commitments = store
            .list_active_commitments(character_id, user_id, limit)
            .await?;
        self.insert_if_current(epoch, |inner| {
            inner.commitments.insert(
                key,
                CachedCommitments {
                    commitments: commitments.clone(),
                    limit,
                },
            );
            evict_fifo(&mut inner.commitments, MAX_SCOPE_ENTRIES);
        });
        Ok(commitments)
    }

    /// Cached pending-candidate queue (status `Pending`), or L2 on miss.
    pub async fn list_pending_candidates(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
    ) -> Result<Vec<PendingCandidate>, MemoryPortError> {
        let key = ScopeKey::new(character_id, None);
        if let Some(cached) = self.inner.read().pending.get(&key) {
            self.count_hit("pending");
            return Ok(cached.clone());
        }
        self.count_miss("pending");
        let epoch = self.epoch.load(Ordering::Acquire);
        let candidates = store
            .list_pending_candidates(character_id, Some(PendingCandidateStatus::Pending))
            .await?;
        self.insert_if_current(epoch, |inner| {
            inner.pending.insert(key, candidates.clone());
            evict_fifo(&mut inner.pending, MAX_SCOPE_ENTRIES);
        });
        Ok(candidates)
    }

    /// Cached reflection memories for a character, or L2 on miss.
    pub async fn get_reflection_memories(
        &self,
        store: &dyn MemoryPort,
        character_id: &str,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        if let Some(cached) = self.inner.read().reflections.get(character_id) {
            self.count_hit("reflections");
            return Ok(cached.clone());
        }
        self.count_miss("reflections");
        let epoch = self.epoch.load(Ordering::Acquire);
        // Loaded directly (mirroring `load_reflection_memories` semantics) so
        // a transient store error degrades to empty without being cached as
        // if it were the true reflection set.
        let items: Vec<MemoryItem> = store
            .get_typed_memories_by_character(
                character_id,
                Some(MemoryKind::Reflection),
                None,
                None,
                50,
                0,
            )
            .await?
            .into_iter()
            .filter(|item| item.status == ene_core::MemoryStatus::Active)
            .collect();
        self.insert_if_current(epoch, |inner| {
            inner
                .reflections
                .insert(character_id.to_string(), items.clone());
            evict_fifo(&mut inner.reflections, MAX_SCOPE_ENTRIES);
        });
        Ok(items)
    }

    /// Insert a miss result only while the cache epoch still matches.
    ///
    /// The epoch check must happen after acquiring the write lock. Checking
    /// before locking leaves a window where an invalidation can clear the
    /// cache and release the lock before this task acquires it, allowing its
    /// pre-invalidation result to be inserted again.
    fn insert_if_current(&self, epoch: u64, insert: impl FnOnce(&mut Inner)) {
        let mut inner = self.inner.write();
        if self.epoch.load(Ordering::Acquire) == epoch {
            insert(&mut inner);
        }
    }

    /// Apply an access bump to cached candidates in place.
    ///
    /// The store row's `access_count` / `last_accessed_at` feed the access
    /// boost during scoring, so a cached copy would drift from the uncached
    /// result after prompt injection. Invalidating instead would clear the hot
    /// entries every turn, which defeats the cache; mirroring the bump keeps
    /// scores identical while preserving hits.
    pub fn refresh_access(&self, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        // Block miss-path inserts that started before the bump from landing
        // pre-bump snapshots after the in-place refresh.
        self.epoch.fetch_add(1, Ordering::AcqRel);
        let now = Utc::now();
        let mut inner = self.inner.write();
        for cached in inner.searches.values_mut() {
            for candidate in cached {
                let Some(id) = candidate.item.id else {
                    continue;
                };
                if ids.contains(&id) {
                    candidate.item.access_count = candidate.item.access_count.saturating_add(1);
                    candidate.item.last_accessed_at = Some(now);
                }
            }
        }
    }

    fn count_hit(&self, section: &str) {
        self.hits.fetch_add(1, Ordering::Relaxed);
        debug!(
            component = "RecallCache",
            section,
            hits = self.hits.load(Ordering::Relaxed),
            misses = self.misses.load(Ordering::Relaxed),
            "L1 recall cache hit"
        );
    }

    fn count_miss(&self, section: &str) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        debug!(
            component = "RecallCache",
            section,
            hits = self.hits.load(Ordering::Relaxed),
            misses = self.misses.load(Ordering::Relaxed),
            "L1 recall cache miss, falling back to L2"
        );
    }
}

fn evict_fifo<K, V>(entries: &mut IndexMap<K, V>, max: usize) {
    while entries.len() > max {
        entries.shift_remove_index(0);
    }
}

/// `MemoryPort` decorator that records whether a recall-relevant write
/// happened, so the post-turn pipeline can invalidate the cache only when it
/// actually changed state. Every-turn unconditional invalidation would clear
/// the hot entries before the next turn, making the L1 tier useless.
///
/// Affect persistence (`upsert_affect_state`) is deliberately not tracked:
/// it runs on every turn and never changes recall data.
pub(crate) struct WriteTrackingPort<'a> {
    inner: &'a dyn MemoryPort,
    wrote: AtomicBool,
    invalidate: Option<(&'a MemoryRecallCache, &'a str)>,
}

impl<'a> WriteTrackingPort<'a> {
    pub(crate) fn new(
        inner: &'a dyn MemoryPort,
        invalidate: Option<(&'a MemoryRecallCache, &'a str)>,
    ) -> Self {
        Self {
            inner,
            wrote: AtomicBool::new(false),
            invalidate,
        }
    }

    fn mark(&self) {
        if self.wrote.swap(true, Ordering::Relaxed) {
            return;
        }
        // Invalidate at the first observed write: the pipeline keeps writing
        // for a while, and any turn in that window must not hit pre-write
        // cache entries while fresh L2 reads return post-write rows.
        if let Some((cache, character_id)) = self.invalidate {
            cache.invalidate_character(character_id);
        }
    }
}

#[async_trait::async_trait]
impl MemoryPort for WriteTrackingPort<'_> {
    async fn insert_typed_memory(
        &self,
        item: &ene_core::NewMemoryItem,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner.insert_typed_memory(item).await
    }

    async fn get_typed_memories_by_character(
        &self,
        character_id: &str,
        kind: Option<MemoryKind>,
        user_id: Option<&str>,
        status: Option<ene_core::MemoryStatus>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        self.inner
            .get_typed_memories_by_character(character_id, kind, user_id, status, limit, offset)
            .await
    }

    async fn list_typed_memories_by_source_prefix(
        &self,
        character_id: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<MemoryItem>, MemoryPortError> {
        self.inner
            .list_typed_memories_by_source_prefix(character_id, prefix, limit)
            .await
    }

    async fn get_active_typed_memory_by_source_ref(
        &self,
        character_id: &str,
        source_ref: &str,
    ) -> Result<Option<MemoryItem>, MemoryPortError> {
        self.inner
            .get_active_typed_memory_by_source_ref(character_id, source_ref)
            .await
    }

    async fn archive_typed_memories_by_source_prefixes(
        &self,
        character_id: &str,
        prefixes: &[&str],
        keep_refs: &std::collections::HashSet<String>,
    ) -> Result<usize, MemoryPortError> {
        let archived = self
            .inner
            .archive_typed_memories_by_source_prefixes(character_id, prefixes, keep_refs)
            .await?;
        if archived > 0 {
            self.mark();
        }
        Ok(archived)
    }

    async fn search(&self, query: &Query<'_>) -> Result<Vec<GatheredCandidate>, MemoryPortError> {
        self.inner.search(query).await
    }

    async fn supersede_typed_memory(
        &self,
        new_item: &ene_core::NewMemoryItem,
        superseded_id: i64,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner
            .supersede_typed_memory(new_item, superseded_id)
            .await
    }

    async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryPortError> {
        self.inner.bump_typed_memory_access(id).await
    }

    async fn set_memory_status(
        &self,
        id: i64,
        new_status: ene_core::MemoryStatus,
    ) -> Result<bool, MemoryPortError> {
        let changed = self.inner.set_memory_status(id, new_status).await?;
        if changed {
            self.mark();
        }
        Ok(changed)
    }

    async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: chrono::DateTime<Utc>,
        half_life_days: f64,
        fade_threshold: f32,
        archive_threshold: f32,
    ) -> Result<ene_core::NaturalDecayReport, MemoryPortError> {
        let report = self
            .inner
            .apply_natural_decay_batch(
                character_id,
                user_id,
                now,
                half_life_days,
                fade_threshold,
                archive_threshold,
            )
            .await?;
        if report.faded_count > 0 || report.archived_count > 0 {
            self.mark();
        }
        Ok(report)
    }

    async fn upsert_memory_embedding(
        &self,
        memory_item_id: i64,
        model_name: &str,
        field: &str,
        embedding: &[f32],
    ) -> Result<(), MemoryPortError> {
        self.mark();
        self.inner
            .upsert_memory_embedding(memory_item_id, model_name, field, embedding)
            .await
    }

    async fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner.insert_pending_candidate(candidate).await
    }

    async fn prune_pending_candidates(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        max_age_days: u32,
        max_per_character: usize,
        now: chrono::DateTime<Utc>,
    ) -> Result<usize, MemoryPortError> {
        let pruned = self
            .inner
            .prune_pending_candidates(character_id, user_id, max_age_days, max_per_character, now)
            .await?;
        if pruned > 0 {
            self.mark();
        }
        Ok(pruned)
    }

    async fn list_pending_candidates(
        &self,
        character_id: &str,
        status_filter: Option<PendingCandidateStatus>,
    ) -> Result<Vec<PendingCandidate>, MemoryPortError> {
        self.inner
            .list_pending_candidates(character_id, status_filter)
            .await
    }

    async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        self.inner
            .list_active_commitments(character_id, user_id, limit)
            .await
    }

    async fn get_affect_state(
        &self,
        character_id: &str,
    ) -> Result<ene_core::AffectState, MemoryPortError> {
        self.inner.get_affect_state(character_id).await
    }

    async fn upsert_affect_state(
        &self,
        affect: &ene_core::AffectState,
    ) -> Result<(), MemoryPortError> {
        self.inner.upsert_affect_state(affect).await
    }

    async fn take_pending_affect_proposal(
        &self,
        character_id: &str,
        user_name: &str,
    ) -> Result<Option<ene_core::PendingAffectProposal>, MemoryPortError> {
        self.inner
            .take_pending_affect_proposal(character_id, user_name)
            .await
    }

    async fn insert_commitment(
        &self,
        new: &ene_core::NewCommitment,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner.insert_commitment(new).await
    }

    async fn supersede_commitment(
        &self,
        id: i64,
        description: &str,
        due_label: Option<&str>,
    ) -> Result<bool, MemoryPortError> {
        let changed = self
            .inner
            .supersede_commitment(id, description, due_label)
            .await?;
        if changed {
            self.mark();
        }
        Ok(changed)
    }

    async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryPortError> {
        let changed = self.inner.complete_commitment(id).await?;
        if changed {
            self.mark();
        }
        Ok(changed)
    }

    async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryPortError> {
        let changed = self.inner.cancel_commitment(id).await?;
        if changed {
            self.mark();
        }
        Ok(changed)
    }

    async fn mark_stale_commitments(
        &self,
        now: chrono::DateTime<Utc>,
    ) -> Result<usize, MemoryPortError> {
        let stale = self.inner.mark_stale_commitments(now).await?;
        if stale > 0 {
            self.mark();
        }
        Ok(stale)
    }

    async fn enqueue_pending_memory_write(
        &self,
        character_id: &str,
        user_id: &str,
        payload: String,
        error_message: String,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner
            .enqueue_pending_memory_write(character_id, user_id, payload, error_message)
            .await
    }

    async fn take_due_pending_memory_writes(
        &self,
        limit: usize,
    ) -> Result<Vec<ene_core::PendingMemoryWrite>, MemoryPortError> {
        self.inner.take_due_pending_memory_writes(limit).await
    }

    async fn complete_pending_memory_write(&self, id: i64) -> Result<(), MemoryPortError> {
        self.mark();
        self.inner.complete_pending_memory_write(id).await
    }

    async fn fail_pending_memory_write(
        &self,
        id: i64,
        error_message: String,
    ) -> Result<ene_core::PendingMemoryWrite, MemoryPortError> {
        self.mark();
        self.inner
            .fail_pending_memory_write(id, error_message)
            .await
    }

    async fn insert_memory_span(
        &self,
        span: &ene_core::NewMemorySpan,
    ) -> Result<i64, MemoryPortError> {
        self.mark();
        self.inner.insert_memory_span(span).await
    }

    async fn get_active_scene_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<ene_core::ActiveSceneSummaryRow>, MemoryPortError> {
        self.inner.get_active_scene_summary(session_id).await
    }

    async fn list_memory_spans_by_session_and_level(
        &self,
        session_id: &str,
        level: i32,
    ) -> Result<Vec<ene_core::NewMemorySpan>, MemoryPortError> {
        self.inner
            .list_memory_spans_by_session_and_level(session_id, level)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_core::{
        AffectAnnotation, AffectState, MemoryConfidence, MemorySalience, MemoryScope, MemorySource,
        MemoryStatus, NewCommitment, NewMemoryItem, NewMemorySpan, PendingMemoryWrite,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn sample_item(id: i64, kind: MemoryKind) -> MemoryItem {
        MemoryItem {
            id: Some(id),
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind,
            title: format!("title {id}"),
            content: format!("content {id}"),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: AffectAnnotation::default(),
            relationship_impact: 0.0,
            access_count: 0,
            last_accessed_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            faded_at: None,
            commitment_id: None,
        }
    }

    fn sample_commitment(id: i64) -> Commitment {
        Commitment {
            id: Some(id),
            character_id: "Ene".into(),
            user_id: "User".into(),
            title: format!("promise {id}"),
            description: "desc".into(),
            due_label: None,
            due_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: ene_core::CommitmentStatus::Active,
            completed_at: None,
        }
    }

    fn sample_query<'a>(
        session_id: &str,
        embedding: Option<&'a [f32]>,
        query_text: &'a str,
    ) -> (String, Query<'a>) {
        (
            session_id.to_string(),
            Query {
                query_text,
                embedding,
                character_id: "Ene",
                user_id: Some("User"),
                model_name: "mock",
                limit: 8,
                similarity_threshold: 0.0,
                candidate_pool_size: 32,
                query_affect: None,
                weights: ene_core::HybridSearchWeights::default(),
                decay_half_life_days: 30.0,
                access_boost_half_life_days: 14.0,
                now: Utc::now(),
                min_score: 0.0,
                commitment_boost: 0.0,
                recent_fallback_limit: 5,
                time_range: None,
                exclude_kinds: vec![],
            },
        )
    }

    /// `MemoryPort` double with canned recall responses and per-method call
    /// counters for proving that L1 hits skip L2.
    #[derive(Debug, Default)]
    struct CountingPort {
        search_result: Mutex<Vec<GatheredCandidate>>,
        commitments: Mutex<Vec<Commitment>>,
        pending: Mutex<Vec<PendingCandidate>>,
        reflections: Mutex<Vec<MemoryItem>>,
        search_calls: AtomicUsize,
        commitment_calls: AtomicUsize,
        pending_calls: AtomicUsize,
        reflection_calls: AtomicUsize,
        gate: Mutex<Option<SearchGate>>,
    }

    use parking_lot::Mutex;

    /// One-shot blocking hook for `search`, letting a test invalidate the
    /// cache while an L2 read is in flight.
    #[derive(Debug)]
    struct SearchGate {
        started: tokio::sync::mpsc::UnboundedSender<String>,
        release: tokio::sync::mpsc::UnboundedReceiver<String>,
    }

    impl CountingPort {
        fn with_canned(candidates: Vec<GatheredCandidate>) -> Self {
            Self {
                search_result: Mutex::new(candidates),
                ..Self::default()
            }
        }
    }

    #[async_trait::async_trait]
    impl MemoryPort for CountingPort {
        async fn insert_typed_memory(&self, _item: &NewMemoryItem) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn get_typed_memories_by_character(
            &self,
            _character_id: &str,
            kind: Option<MemoryKind>,
            _user_id: Option<&str>,
            _status: Option<MemoryStatus>,
            _limit: usize,
            _offset: usize,
        ) -> Result<Vec<MemoryItem>, MemoryPortError> {
            if kind == Some(MemoryKind::Reflection) {
                self.reflection_calls.fetch_add(1, AtomicOrdering::Relaxed);
                return Ok(self.reflections.lock().clone());
            }
            Ok(vec![])
        }

        async fn list_typed_memories_by_source_prefix(
            &self,
            _character_id: &str,
            _prefix: &str,
            _limit: usize,
        ) -> Result<Vec<MemoryItem>, MemoryPortError> {
            Ok(vec![])
        }

        async fn get_active_typed_memory_by_source_ref(
            &self,
            _character_id: &str,
            _source_ref: &str,
        ) -> Result<Option<MemoryItem>, MemoryPortError> {
            Ok(None)
        }

        async fn archive_typed_memories_by_source_prefixes(
            &self,
            _character_id: &str,
            _prefixes: &[&str],
            _keep_refs: &HashSet<String>,
        ) -> Result<usize, MemoryPortError> {
            Ok(0)
        }

        async fn search(
            &self,
            _query: &Query<'_>,
        ) -> Result<Vec<GatheredCandidate>, MemoryPortError> {
            self.search_calls.fetch_add(1, AtomicOrdering::Relaxed);
            let gate = self.gate.lock().take();
            if let Some(mut gate) = gate {
                drop(gate.started.send("started".to_string()));
                drop(gate.release.recv().await);
            }
            Ok(self.search_result.lock().clone())
        }

        async fn supersede_typed_memory(
            &self,
            _new_item: &NewMemoryItem,
            _superseded_id: i64,
        ) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn bump_typed_memory_access(&self, _id: i64) -> Result<bool, MemoryPortError> {
            Ok(true)
        }

        async fn set_memory_status(
            &self,
            _id: i64,
            _new_status: MemoryStatus,
        ) -> Result<bool, MemoryPortError> {
            Ok(true)
        }

        async fn apply_natural_decay_batch(
            &self,
            _character_id: &str,
            _user_id: Option<&str>,
            _now: chrono::DateTime<Utc>,
            _half_life_days: f64,
            _fade_threshold: f32,
            _archive_threshold: f32,
        ) -> Result<ene_core::NaturalDecayReport, MemoryPortError> {
            Ok(ene_core::NaturalDecayReport::default())
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

        async fn insert_pending_candidate(
            &self,
            _candidate: PendingCandidate,
        ) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn prune_pending_candidates(
            &self,
            _character_id: &str,
            _user_id: Option<&str>,
            _max_age_days: u32,
            _max_per_character: usize,
            _now: chrono::DateTime<Utc>,
        ) -> Result<usize, MemoryPortError> {
            Ok(0)
        }

        async fn list_pending_candidates(
            &self,
            _character_id: &str,
            _status_filter: Option<PendingCandidateStatus>,
        ) -> Result<Vec<PendingCandidate>, MemoryPortError> {
            self.pending_calls.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(self.pending.lock().clone())
        }

        async fn list_active_commitments(
            &self,
            _character_id: &str,
            _user_id: Option<&str>,
            limit: usize,
        ) -> Result<Vec<Commitment>, MemoryPortError> {
            self.commitment_calls.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(self
                .commitments
                .lock()
                .iter()
                .take(limit)
                .cloned()
                .collect())
        }

        async fn get_affect_state(
            &self,
            _character_id: &str,
        ) -> Result<AffectState, MemoryPortError> {
            Ok(AffectState::neutral("Ene"))
        }

        async fn upsert_affect_state(&self, _affect: &AffectState) -> Result<(), MemoryPortError> {
            Ok(())
        }

        async fn take_pending_affect_proposal(
            &self,
            _character_id: &str,
            _user_name: &str,
        ) -> Result<Option<ene_core::PendingAffectProposal>, MemoryPortError> {
            Ok(None)
        }

        async fn insert_commitment(&self, _new: &NewCommitment) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn supersede_commitment(
            &self,
            _id: i64,
            _description: &str,
            _due_label: Option<&str>,
        ) -> Result<bool, MemoryPortError> {
            Ok(true)
        }

        async fn complete_commitment(&self, _id: i64) -> Result<bool, MemoryPortError> {
            Ok(true)
        }

        async fn cancel_commitment(&self, _id: i64) -> Result<bool, MemoryPortError> {
            Ok(true)
        }

        async fn mark_stale_commitments(
            &self,
            _now: chrono::DateTime<Utc>,
        ) -> Result<usize, MemoryPortError> {
            Ok(0)
        }

        async fn enqueue_pending_memory_write(
            &self,
            _character_id: &str,
            _user_id: &str,
            _payload: String,
            _error_message: String,
        ) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn take_due_pending_memory_writes(
            &self,
            _limit: usize,
        ) -> Result<Vec<PendingMemoryWrite>, MemoryPortError> {
            Ok(vec![])
        }

        async fn complete_pending_memory_write(&self, _id: i64) -> Result<(), MemoryPortError> {
            Ok(())
        }

        async fn fail_pending_memory_write(
            &self,
            _id: i64,
            _error_message: String,
        ) -> Result<PendingMemoryWrite, MemoryPortError> {
            Err(MemoryPortError::Other("not used in tests".into()))
        }

        async fn insert_memory_span(&self, _span: &NewMemorySpan) -> Result<i64, MemoryPortError> {
            Ok(0)
        }

        async fn get_active_scene_summary(
            &self,
            _session_id: &str,
        ) -> Result<Option<ene_core::ActiveSceneSummaryRow>, MemoryPortError> {
            Ok(None)
        }

        async fn list_memory_spans_by_session_and_level(
            &self,
            _session_id: &str,
            _level: i32,
        ) -> Result<Vec<NewMemorySpan>, MemoryPortError> {
            Ok(vec![])
        }
    }

    #[tokio::test]
    async fn search_hit_skips_store_and_is_identical() {
        let item = sample_item(1, MemoryKind::Semantic);
        let candidate = GatheredCandidate {
            item,
            vector_similarity: 0.9,
            sources: vec![ene_core::MemoryCandidateSource::Vector],
        };
        let store = CountingPort::with_canned(vec![candidate.clone()]);
        let cache = MemoryRecallCache::new();
        let (session_id, query) = sample_query("sess-1", Some(&[1.0, 0.0, 0.0]), "coffee");

        let first = cache.search(&store, &session_id, &query).await.unwrap();
        let second = cache.search(&store, &session_id, &query).await.unwrap();

        assert_eq!(first, second);
        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 1);
        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn search_key_distinguishes_session_embedding_and_text() {
        let store = CountingPort::with_canned(vec![]);
        let cache = MemoryRecallCache::new();

        let (session_a, query) = sample_query("sess-a", Some(&[1.0, 0.0, 0.0]), "coffee");
        let (_session_b, query_b) = sample_query("sess-a", Some(&[1.0, 0.0, 0.0]), "tea");
        let (session_b, query_other_session) =
            sample_query("sess-b", Some(&[1.0, 0.0, 0.0]), "coffee");
        let (_, query_other_embedding) = sample_query("sess-a", Some(&[0.0, 1.0, 0.0]), "coffee");

        cache.search(&store, &session_a, &query).await.unwrap();
        cache.search(&store, &session_a, &query_b).await.unwrap();
        cache
            .search(&store, &session_b, &query_other_session)
            .await
            .unwrap();
        cache
            .search(&store, &session_a, &query_other_embedding)
            .await
            .unwrap();
        cache.search(&store, &session_a, &query).await.unwrap();

        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 4);
    }

    #[tokio::test]
    async fn commitment_cache_serves_smaller_limits_and_grows() {
        let store = CountingPort::default();
        store
            .commitments
            .lock()
            .extend((1..=20).map(sample_commitment));
        let cache = MemoryRecallCache::new();

        let first = cache
            .list_active_commitments(&store, "Ene", Some("User"), 8)
            .await
            .unwrap();
        let smaller = cache
            .list_active_commitments(&store, "Ene", Some("User"), 5)
            .await
            .unwrap();
        assert_eq!(smaller, first[..5]);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 1);

        let larger = cache
            .list_active_commitments(&store, "Ene", Some("User"), 16)
            .await
            .unwrap();
        assert_eq!(larger.len(), 16);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 2);

        let again = cache
            .list_active_commitments(&store, "Ene", Some("User"), 16)
            .await
            .unwrap();
        assert_eq!(again, larger);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 2);
    }

    #[tokio::test]
    async fn write_invalidation_drops_character_entries() {
        let store = CountingPort::with_canned(vec![]);
        store.commitments.lock().push(sample_commitment(1));
        store.pending.lock().push(PendingCandidate {
            id: 1,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind: MemoryKind::Episodic,
            title: "pending".into(),
            content: "body".into(),
            confidence: 1.0,
            reason_detail: "test".into(),
            existing_memory_title: None,
            existing_memory_id: None,
            source_quote: "pending".into(),
            status: PendingCandidateStatus::Pending,
            created_at: Utc::now(),
        });
        store
            .reflections
            .lock()
            .push(sample_item(2, MemoryKind::Reflection));
        let cache = MemoryRecallCache::new();
        let (session_id, query) = sample_query("sess-1", Some(&[1.0, 0.0, 0.0]), "coffee");

        cache.search(&store, &session_id, &query).await.unwrap();
        cache
            .list_active_commitments(&store, "Ene", Some("User"), 8)
            .await
            .unwrap();
        cache.list_pending_candidates(&store, "Ene").await.unwrap();
        cache.get_reflection_memories(&store, "Ene").await.unwrap();
        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(store.pending_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(store.reflection_calls.load(AtomicOrdering::Relaxed), 1);

        cache.invalidate_character("Ene");

        cache.search(&store, &session_id, &query).await.unwrap();
        cache
            .list_active_commitments(&store, "Ene", Some("User"), 8)
            .await
            .unwrap();
        cache.list_pending_candidates(&store, "Ene").await.unwrap();
        cache.get_reflection_memories(&store, "Ene").await.unwrap();
        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(store.pending_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(store.reflection_calls.load(AtomicOrdering::Relaxed), 2);
    }

    #[tokio::test]
    async fn invalidate_character_keeps_other_characters() {
        let store = CountingPort::with_canned(vec![]);
        let cache = MemoryRecallCache::new();
        let (session_id, query) = sample_query("sess-1", Some(&[1.0, 0.0, 0.0]), "coffee");
        let other = Query {
            character_id: "Other",
            ..query.clone()
        };

        cache.search(&store, &session_id, &query).await.unwrap();
        cache.search(&store, &session_id, &other).await.unwrap();
        cache
            .list_active_commitments(&store, "Other", Some("User"), 8)
            .await
            .unwrap();
        cache
            .list_pending_candidates(&store, "Other")
            .await
            .unwrap();
        cache
            .get_reflection_memories(&store, "Other")
            .await
            .unwrap();
        cache.invalidate_character("Ene");
        cache.search(&store, &session_id, &other).await.unwrap();
        cache
            .list_active_commitments(&store, "Other", Some("User"), 8)
            .await
            .unwrap();
        cache
            .list_pending_candidates(&store, "Other")
            .await
            .unwrap();
        cache
            .get_reflection_memories(&store, "Other")
            .await
            .unwrap();

        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(store.pending_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(store.reflection_calls.load(AtomicOrdering::Relaxed), 1);
    }

    #[tokio::test]
    async fn stale_read_does_not_poison_cache_after_invalidation() {
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = tokio::sync::mpsc::unbounded_channel();
        let store = std::sync::Arc::new(CountingPort {
            gate: Mutex::new(Some(SearchGate {
                started: started_tx,
                release: release_rx,
            })),
            ..CountingPort::default()
        });
        let cache = std::sync::Arc::new(MemoryRecallCache::new());
        let (session_id, query) = sample_query("sess", Some(&[1.0, 0.0, 0.0]), "coffee");

        let read_cache = cache.clone();
        let read_store = store.clone();
        let read_query = query.clone();
        let read = tokio::spawn(async move {
            read_cache
                .search(read_store.as_ref(), "sess", &read_query)
                .await
                .unwrap();
        });

        started_rx.recv().await.unwrap();
        cache.invalidate_all();
        drop(release_tx.send("go".to_string()));
        read.await.unwrap();

        // The in-flight read must not have re-inserted its pre-invalidation
        // snapshot: the next search goes to L2 again.
        assert!(cache.inner.read().searches.is_empty());
        cache
            .search(store.as_ref(), &session_id, &query)
            .await
            .unwrap();
        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 2);
    }

    #[test]
    fn miss_insert_rechecks_epoch_after_acquiring_write_lock() {
        let cache = MemoryRecallCache::new();
        let epoch = cache.epoch.load(Ordering::Acquire);
        let inserted = std::sync::atomic::AtomicBool::new(false);

        // Model the invalidation completing after the miss-path's initial
        // epoch read but before its insert lock is acquired.
        cache.epoch.fetch_add(1, Ordering::AcqRel);
        cache.insert_if_current(epoch, |_| {
            inserted.store(true, AtomicOrdering::Relaxed);
        });

        assert!(!inserted.load(AtomicOrdering::Relaxed));
    }

    #[tokio::test]
    async fn session_split_invalidates_all_entries() {
        let store = CountingPort::with_canned(vec![]);
        store.commitments.lock().push(sample_commitment(1));
        let cache = MemoryRecallCache::new();
        let (session_id, query) = sample_query("sess-old", Some(&[1.0, 0.0, 0.0]), "coffee");

        cache.search(&store, &session_id, &query).await.unwrap();
        cache
            .list_active_commitments(&store, "Ene", Some("User"), 8)
            .await
            .unwrap();

        // Session split: fresh id must never reuse the old session's gather.
        cache.invalidate_all();
        let (new_session, query_new) = sample_query("sess-new", Some(&[1.0, 0.0, 0.0]), "coffee");
        cache
            .search(&store, &new_session, &query_new)
            .await
            .unwrap();
        cache
            .list_active_commitments(&store, "Ene", Some("User"), 8)
            .await
            .unwrap();

        assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(store.commitment_calls.load(AtomicOrdering::Relaxed), 2);
        assert_eq!(cache.stats().invalidations, 1);
    }

    #[test]
    fn eviction_bounds_search_entries() {
        let store = CountingPort::with_canned(vec![]);
        let cache = MemoryRecallCache::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for i in 0..(MAX_SEARCH_ENTRIES + 8) {
                let embedding = [i as f32];
                let query_text = format!("q{i}");
                let (session_id, query) = sample_query("sess", Some(&embedding), &query_text);
                cache.search(&store, &session_id, &query).await.unwrap();
            }
            assert_eq!(cache.inner.read().searches.len(), MAX_SEARCH_ENTRIES);
        });
    }

    #[test]
    fn refresh_access_mirrors_store_bump() {
        let store = CountingPort::with_canned(vec![]);
        let cache = MemoryRecallCache::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let candidate = GatheredCandidate {
                item: sample_item(7, MemoryKind::Episodic),
                vector_similarity: 0.5,
                sources: vec![],
            };
            *store.search_result.lock() = vec![candidate.clone()];
            let (session_id, query) = sample_query("sess", Some(&[1.0, 0.0, 0.0]), "coffee");
            cache.search(&store, &session_id, &query).await.unwrap();

            cache.refresh_access(&[7]);

            let cached = cache.search(&store, &session_id, &query).await.unwrap();
            assert_eq!(cached[0].item.access_count, 1);
            assert!(cached[0].item.last_accessed_at.is_some());
            // The bump is mirrored, not invalidating: the gather stayed cached.
            assert_eq!(store.search_calls.load(AtomicOrdering::Relaxed), 1);
        });
    }

    #[tokio::test]
    async fn write_tracking_port_marks_mutations_only() {
        let inner = CountingPort::default();
        let cache = MemoryRecallCache::new();
        let tracker = WriteTrackingPort::new(&inner, Some((&cache, "Ene")));
        assert_eq!(cache.stats().invalidations, 0);

        tracker
            .search(&sample_query("s", None, "q").1)
            .await
            .unwrap();
        tracker
            .list_active_commitments("Ene", Some("User"), 8)
            .await
            .unwrap();
        tracker.bump_typed_memory_access(1).await.unwrap();
        assert_eq!(cache.stats().invalidations, 0);

        tracker
            .set_memory_status(1, MemoryStatus::UserDeleted)
            .await
            .unwrap();
        assert_eq!(cache.stats().invalidations, 1);
    }

    #[tokio::test]
    async fn cached_recall_skips_l2_on_repeat_turn() {
        use crate::config::MindConfig;
        use crate::recall::{ExecuteRecallInput, execute_hybrid_recall};

        let store = CountingPort::default();
        store.commitments.lock().push(sample_commitment(1));
        store.pending.lock().push(PendingCandidate {
            id: 2,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind: MemoryKind::Episodic,
            title: "coffee plan".into(),
            content: "user wants coffee".into(),
            confidence: 0.8,
            reason_detail: "test".into(),
            existing_memory_title: None,
            existing_memory_id: None,
            source_quote: "coffee".into(),
            status: PendingCandidateStatus::Pending,
            created_at: Utc::now(),
        });
        store
            .reflections
            .lock()
            .push(sample_item(3, MemoryKind::Reflection));
        *store.search_result.lock() = vec![GatheredCandidate {
            item: sample_item(4, MemoryKind::Preference),
            vector_similarity: 0.9,
            sources: vec![ene_core::MemoryCandidateSource::Vector],
        }];

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.reflection.enabled = true;

        let cache = MemoryRecallCache::new();
        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "What does the user like to drink?",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            affect: None,
            cache: Some(&cache),
            session_id: "sess",
        };

        let (_, first) = execute_hybrid_recall(&config, &input).await.unwrap();
        let calls = (
            store.search_calls.load(AtomicOrdering::Relaxed),
            store.commitment_calls.load(AtomicOrdering::Relaxed),
            store.pending_calls.load(AtomicOrdering::Relaxed),
            store.reflection_calls.load(AtomicOrdering::Relaxed),
        );
        assert_eq!(calls, (1, 1, 1, 1));

        let (_, second) = execute_hybrid_recall(&config, &input).await.unwrap();
        assert_eq!(
            (
                store.search_calls.load(AtomicOrdering::Relaxed),
                store.commitment_calls.load(AtomicOrdering::Relaxed),
                store.pending_calls.load(AtomicOrdering::Relaxed),
                store.reflection_calls.load(AtomicOrdering::Relaxed),
            ),
            calls,
            "repeat turn must be served entirely from L1"
        );
        assert_eq!(first, second, "cached recall must match uncached recall");
        let stats = cache.stats();
        assert_eq!(stats.hits, 4);
        assert_eq!(stats.misses, 4);
    }

    #[test]
    fn reset_session_invalidates_the_shared_cache() {
        let mut session = crate::session::ConversationSession::new();
        let cache = session.memory.recall_cache.clone().unwrap();
        cache.invalidate_character("Ene");
        assert_eq!(cache.stats().invalidations, 1);

        session.reset_session();

        assert_eq!(
            cache.stats().invalidations,
            2,
            "session split must clear the L1 cache in place"
        );
    }
}
