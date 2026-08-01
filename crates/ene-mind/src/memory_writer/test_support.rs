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
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "test support for memory-writer tests uses deliberate id/access-count arithmetic and indexes its in-memory pending-candidate buffer"
)]
use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ene_core::{
    ActiveSceneSummaryRow, AffectState, Commitment, GatheredCandidate, MemoryItem, MemoryKind,
    MemoryPort, MemoryPortError, MemoryStatus, NaturalDecayReport, NewCommitment, NewMemoryItem,
    NewMemorySpan, PendingAffectProposal, PendingCandidate, PendingMemoryWrite, Query,
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

    async fn search(&self, _query: &Query<'_>) -> Result<Vec<GatheredCandidate>, MemoryPortError> {
        // No test using this double exercises hybrid recall scoring; kept
        // trivially empty rather than reimplementing `ene-store`'s gather.
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
        _fade_threshold: f32,
        _archive_threshold: f32,
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

    async fn insert_pending_candidate(
        &self,
        candidate: PendingCandidate,
    ) -> Result<i64, MemoryPortError> {
        let id = self.alloc_id();
        let mut stored = candidate;
        stored.id = id;
        self.pending.lock().push(stored);
        Ok(id)
    }

    async fn prune_pending_candidates(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        max_age_days: u32,
        max_per_character: usize,
        now: DateTime<Utc>,
    ) -> Result<usize, MemoryPortError> {
        // Mirror the store's visibility rule: the character plus, when a user
        // is given, that user's rows and character-shared rows (`user_id = ""`).
        let in_scope = |c: &PendingCandidate| {
            c.character_id == character_id
                && user_id.is_none_or(|uid| c.user_id == uid || c.user_id.is_empty())
        };

        let mut pending = self.pending.lock();
        let before = pending.len();

        if max_age_days > 0 {
            let cutoff = now
                .checked_sub_signed(chrono::Duration::days(i64::from(max_age_days)))
                .unwrap_or(now);
            pending.retain(|c| !(in_scope(c) && c.created_at < cutoff));
        }

        if max_per_character > 0 {
            let mut indices: Vec<usize> = pending
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    in_scope(c) && c.status == ene_core::PendingCandidateStatus::Pending
                })
                .map(|(i, _)| i)
                .collect();
            // Oldest first; drop the overflow beyond the cap.
            indices.sort_by(|&a, &b| pending[a].created_at.cmp(&pending[b].created_at));
            if indices.len() > max_per_character {
                let overflow = indices.len() - max_per_character;
                let drop: HashSet<usize> = indices.into_iter().take(overflow).collect();
                let mut idx = 0usize;
                pending.retain(|_| {
                    let keep = !drop.contains(&idx);
                    idx += 1;
                    keep
                });
            }
        }

        Ok(before - pending.len())
    }

    async fn list_active_commitments(
        &self,
        _character_id: &str,
        _user_id: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<Commitment>, MemoryPortError> {
        Ok(Vec::new())
    }

    // ── Affect state (#309) ────────────────────────────────────────────────

    async fn get_affect_state(&self, character_id: &str) -> Result<AffectState, MemoryPortError> {
        Ok(AffectState::neutral(character_id))
    }

    async fn upsert_affect_state(&self, _affect: &AffectState) -> Result<(), MemoryPortError> {
        Ok(())
    }

    async fn take_pending_affect_proposal(
        &self,
        _character_id: &str,
        _user_name: &str,
    ) -> Result<Option<PendingAffectProposal>, MemoryPortError> {
        Ok(None)
    }

    // ── Commitment CRUD (#309) ─────────────────────────────────────────────

    async fn insert_commitment(&self, _new: &NewCommitment) -> Result<i64, MemoryPortError> {
        Ok(self.alloc_id())
    }

    async fn supersede_commitment(
        &self,
        _id: i64,
        _description: &str,
        _due_label: Option<&str>,
    ) -> Result<bool, MemoryPortError> {
        Ok(false)
    }

    async fn complete_commitment(&self, _id: i64) -> Result<bool, MemoryPortError> {
        Ok(false)
    }

    async fn cancel_commitment(&self, _id: i64) -> Result<bool, MemoryPortError> {
        Ok(false)
    }

    async fn mark_stale_commitments(
        &self,
        _now: chrono::DateTime<chrono::Utc>,
    ) -> Result<usize, MemoryPortError> {
        Ok(0)
    }

    // ── Pending memory writes (#309) ───────────────────────────────────────

    async fn enqueue_pending_memory_write(
        &self,
        _character_id: &str,
        _user_id: &str,
        _payload: String,
        _error_message: String,
    ) -> Result<i64, MemoryPortError> {
        Ok(self.alloc_id())
    }

    async fn take_due_pending_memory_writes(
        &self,
        _limit: usize,
    ) -> Result<Vec<PendingMemoryWrite>, MemoryPortError> {
        Ok(Vec::new())
    }

    async fn complete_pending_memory_write(&self, _id: i64) -> Result<(), MemoryPortError> {
        Ok(())
    }

    async fn fail_pending_memory_write(
        &self,
        id: i64,
        error_message: String,
    ) -> Result<PendingMemoryWrite, MemoryPortError> {
        Err(MemoryPortError::Backend(format!(
            "pending write {id} not found in test double: {error_message}"
        )))
    }

    // ── Memory spans / compression (#309) ──────────────────────────────────

    async fn insert_memory_span(&self, _span: &NewMemorySpan) -> Result<i64, MemoryPortError> {
        Ok(self.alloc_id())
    }

    async fn get_active_scene_summary(
        &self,
        _session_id: &str,
    ) -> Result<Option<ActiveSceneSummaryRow>, MemoryPortError> {
        Ok(None)
    }

    async fn list_memory_spans_by_session_and_level(
        &self,
        _session_id: &str,
        _level: i32,
    ) -> Result<Vec<NewMemorySpan>, MemoryPortError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::InMemoryMemoryPort;
    use chrono::{DateTime, Duration, Utc};
    use ene_core::{MemoryKind, MemoryPort, PendingCandidate, PendingCandidateStatus};

    fn candidate(
        character_id: &str,
        user_id: &str,
        title: &str,
        created_at: DateTime<Utc>,
        status: PendingCandidateStatus,
    ) -> PendingCandidate {
        PendingCandidate {
            id: 0,
            character_id: character_id.to_string(),
            user_id: user_id.to_string(),
            title: title.to_string(),
            content: format!("{title} content"),
            kind: MemoryKind::Preference,
            confidence: 0.8,
            reason_detail: String::new(),
            existing_memory_title: None,
            existing_memory_id: None,
            source_quote: String::new(),
            status,
            created_at,
        }
    }

    #[tokio::test]
    async fn prune_age_removes_old_rows_of_any_status() {
        let port = InMemoryMemoryPort::new();
        let now = Utc::now();
        let old = now - Duration::days(30);

        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "old pending",
            old,
            PendingCandidateStatus::Pending,
        ))
        .await
        .unwrap();
        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "old approved",
            old,
            PendingCandidateStatus::Approved,
        ))
        .await
        .unwrap();
        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "fresh",
            now,
            PendingCandidateStatus::Pending,
        ))
        .await
        .unwrap();

        let removed = port
            .prune_pending_candidates("ene", Some("u1"), 14, 0, now)
            .await
            .unwrap();
        assert_eq!(removed, 2);

        let remaining = port.pending_candidates();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].title, "fresh");
    }

    #[tokio::test]
    async fn prune_count_cap_drops_oldest_pending_overflow() {
        let port = InMemoryMemoryPort::new();
        let now = Utc::now();
        for i in 0..5i64 {
            // i = 0 is the oldest.
            let created = now - Duration::minutes(5 - i);
            port.insert_pending_candidate(candidate(
                "ene",
                "u1",
                &format!("cand {i}"),
                created,
                PendingCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        }
        // A resolved row that must not count toward the pending cap.
        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "approved",
            now,
            PendingCandidateStatus::Approved,
        ))
        .await
        .unwrap();

        let removed = port
            .prune_pending_candidates("ene", Some("u1"), 0, 3, now)
            .await
            .unwrap();
        assert_eq!(removed, 2);

        let remaining = port.pending_candidates();
        // 3 pending survivors + the untouched approved row.
        assert_eq!(remaining.len(), 4);
        let mut pending_titles: Vec<_> = remaining
            .iter()
            .filter(|c| c.status == PendingCandidateStatus::Pending)
            .map(|c| c.title.clone())
            .collect();
        pending_titles.sort();
        assert_eq!(pending_titles, vec!["cand 2", "cand 3", "cand 4"]);
    }

    #[tokio::test]
    async fn prune_is_character_scoped() {
        let port = InMemoryMemoryPort::new();
        let now = Utc::now();
        for i in 0..5i64 {
            port.insert_pending_candidate(candidate(
                "ene",
                "u1",
                &format!("ene {i}"),
                now - Duration::minutes(5 - i),
                PendingCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        }
        port.insert_pending_candidate(candidate(
            "mira",
            "u1",
            "mira 0",
            now,
            PendingCandidateStatus::Pending,
        ))
        .await
        .unwrap();

        let removed = port
            .prune_pending_candidates("ene", Some("u1"), 0, 2, now)
            .await
            .unwrap();
        assert_eq!(removed, 3);

        let remaining = port.pending_candidates();
        assert_eq!(
            remaining.iter().filter(|c| c.character_id == "ene").count(),
            2
        );
        assert_eq!(
            remaining
                .iter()
                .filter(|c| c.character_id == "mira")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn prune_count_cap_is_user_scoped() {
        let port = InMemoryMemoryPort::new();
        let now = Utc::now();
        for i in 0..5i64 {
            port.insert_pending_candidate(candidate(
                "ene",
                "alice",
                &format!("alice {i}"),
                now - Duration::minutes(5 - i),
                PendingCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        }
        for i in 0..2i64 {
            port.insert_pending_candidate(candidate(
                "ene",
                "bob",
                &format!("bob {i}"),
                now - Duration::minutes(2 - i),
                PendingCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        }

        // Cap as alice: only alice's queue overflows (5 -> 2); bob is intact.
        let removed = port
            .prune_pending_candidates("ene", Some("alice"), 0, 2, now)
            .await
            .unwrap();
        assert_eq!(removed, 3);

        let remaining = port.pending_candidates();
        assert_eq!(remaining.iter().filter(|c| c.user_id == "alice").count(), 2);
        assert_eq!(remaining.iter().filter(|c| c.user_id == "bob").count(), 2);
    }

    #[tokio::test]
    async fn prune_both_limits_do_not_double_count() {
        let port = InMemoryMemoryPort::new();
        let now = Utc::now();
        let old = now - Duration::days(30);
        // Two ancient pending rows (age pass removes them) ...
        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "ancient 0",
            old,
            PendingCandidateStatus::Pending,
        ))
        .await
        .unwrap();
        port.insert_pending_candidate(candidate(
            "ene",
            "u1",
            "ancient 1",
            old,
            PendingCandidateStatus::Pending,
        ))
        .await
        .unwrap();
        // ... plus four fresh pending rows.
        for i in 0..4i64 {
            port.insert_pending_candidate(candidate(
                "ene",
                "u1",
                &format!("fresh {i}"),
                now - Duration::minutes(4 - i),
                PendingCandidateStatus::Pending,
            ))
            .await
            .unwrap();
        }

        // Age pass removes the 2 ancient rows; the count pass then sees 4 fresh
        // pending rows and caps to 3, removing 1 more. Total = 3, not 4.
        let removed = port
            .prune_pending_candidates("ene", Some("u1"), 14, 3, now)
            .await
            .unwrap();
        assert_eq!(removed, 3);
        assert_eq!(port.pending_candidates().len(), 3);
    }
}
