//! Companion Commitment Ledger — promises, tasks, and follow-ups.
//!
//! The `commitments` table is the sole source of truth for commitment lifecycle
//! and prompt injection. Typed `MemoryKind::Commitment` rows are optional
//! references (`typed_memories.commitment_id`) and are not dual-written from
//! arbiter persist.
//!
//! **Contradiction semantics:** this ledger is where a rephrased or
//! rescheduled commitment is *merged* into the existing row (title-keyed
//! matching) instead of both versions surviving as separate valid
//! commitments. The typed-memory arbiter additionally treats `Commitment` as a
//! contradiction-checked kind (`MemoryKind::is_contradiction_kind`) as defense
//! in depth for any direct typed-write path; see the per-kind policy table in
//! `ene-core::MemoryKind`.

#![expect(
    clippy::indexing_slicing,
    reason = "history/token helpers index into bounds-checked conversational buffers"
)]

use ene_ai::EmbeddingProvider;
use ene_core::{ActiveCommitmentPrompt, Commitment, MemoryPort};
use ene_core::{CommitmentStatus, MemoryKind, NewCommitment};
use tracing::{debug, warn};

use crate::error::CognitionError;
use crate::memory_writer::candidate::MemoryCandidate;
use crate::memory_writer::{AppliedDecision, ArbiterContext, MemoryArbiter};
use crate::title_match::{TitleMatchMode, TitleMatcher};

mod due;

pub use due::parse_due_at;

/// Companion Commitment Ledger: promises, tasks, follow-ups.
///
/// # No in-memory cache
///
/// This ledger is **stateless** — the struct has no fields. Every operation
/// (`apply_commitment_candidates`, `list_active`, `complete`, `cancel`,
/// `mark_stale_overdue`) takes `&dyn MemoryPort` and reads or writes the
/// `commitments` table on each call. The runtime actor likewise holds no
/// commitment snapshot: it re-reads `list_active_commitments` from the store
/// at prompt-injection time and only applies the pure
/// [`Self::active_prompt_candidates`] mapping to those fresh rows.
///
/// Consequence: consumers may complete/cancel commitments directly
/// through the memory store (e.g. the desktop UI's commitment buttons) with
/// no actor-side cache to desync. The next prompt injection reads the updated
/// rows, so no mailbox round-trip is required for consistency.
#[derive(Debug, Default)]
pub struct CommitmentLedger;

/// Context required when writing ledger rows from commitment candidates.
///
/// Carries the optional embedding provider used to match commitments by title
/// *similarity* rather than exact string equality. When `embedder` is
/// `None`, matching degrades to the deterministic exact-title fallback so the
/// ledger keeps working without an embedding model.
#[derive(Clone)]
pub struct CommitmentSyncContext<'a> {
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: &'a str,
    /// Embedding provider for fuzzy title matching. `None` falls back to
    /// exact normalized-title equality.
    pub embedder: Option<&'a dyn EmbeddingProvider>,
    /// Minimum title-embedding cosine similarity for two commitments to be
    /// treated as the same one. Only consulted on the embedding path.
    pub title_similarity_threshold: f32,
    /// Maximum active ledger rows loaded for title matching in one apply batch.
    ///
    /// Defaults to [`MindMemoryLimitsConfig::commitment_active_match_limit`](crate::config::MindMemoryLimitsConfig::commitment_active_match_limit)'s
    /// default (`4096`).
    pub active_match_limit: usize,
}

impl Default for CommitmentSyncContext<'_> {
    fn default() -> Self {
        Self {
            character_id: "",
            user_id: "",
            embedder: None,
            title_similarity_threshold: f32::default(),
            active_match_limit: crate::config::MindMemoryLimitsConfig::default()
                .commitment_active_match_limit,
        }
    }
}

impl std::fmt::Debug for CommitmentSyncContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommitmentSyncContext")
            .field("character_id", &self.character_id)
            .field("user_id", &self.user_id)
            .field("has_embedder", &self.embedder.is_some())
            .field(
                "title_similarity_threshold",
                &self.title_similarity_threshold,
            )
            .field("active_match_limit", &self.active_match_limit)
            .finish()
    }
}

impl CommitmentLedger {
    /// Persist commitment candidates to the ledger first (sole `SoT`).
    ///
    /// Does **not** insert typed `MemoryKind::Commitment` bodies. Deletion-style
    /// candidates (`should_persist = false`) cancel matching active ledger rows
    /// by title. Matching is by title-embedding similarity when an embedder is
    /// configured, falling back to exact normalized-title equality
    /// otherwise.
    pub async fn apply_commitment_candidates(
        store: &dyn MemoryPort,
        ctx: &CommitmentSyncContext<'_>,
        candidates: &[MemoryCandidate],
    ) -> Result<Vec<i64>, CognitionError> {
        let mut inserted = Vec::new();

        let mut matcher = TitleMatcher::new(
            ctx.embedder,
            ctx.title_similarity_threshold,
            "CommitmentLedger",
        );

        // Read the active set once and mirror each write into it, rather than
        // re-listing (up to `active_match_limit` rows) per candidate. A
        // candidate still sees what earlier candidates in the same batch did,
        // which is the only reason the read was inside the loop.
        let mut active = list_active_for_match(store, ctx).await?;

        for candidate in candidates {
            if candidate.kind != MemoryKind::Commitment {
                continue;
            }

            if !candidate.should_persist {
                Self::cancel_matching(store, &mut matcher, &candidate.title, &mut active).await?;
                continue;
            }

            if let Some(idx) = Self::find_best_match(&mut matcher, &candidate.title, &active).await
            {
                // Same commitment exists — supersede (update description/due_label) if content differs.
                let existing = &active[idx];
                let content_changed = existing.description != candidate.content;
                let due_changed =
                    existing.due_label.as_deref() != candidate.commitment_due.as_deref();
                if content_changed || due_changed {
                    let due_at = candidate
                        .commitment_due
                        .as_deref()
                        .and_then(|due| parse_due_at(chrono::Utc::now(), due));
                    if let Some(id) = existing.id {
                        store
                            .supersede_commitment(
                                id,
                                &candidate.content,
                                candidate.commitment_due.as_deref(),
                                due_at,
                            )
                            .await
                            .map_err(CognitionError::MemoryPort)?;
                        debug!(
                            component = "CommitmentLedger",
                            commitment_id = id,
                            title = %candidate.title,
                            "superseded active commitment (description/due_label updated)"
                        );
                        let row = &mut active[idx];
                        row.description.clone_from(&candidate.content);
                        row.due_label.clone_from(&candidate.commitment_due);
                        row.due_at = due_at;
                        inserted.push(id);
                    }
                } else {
                    debug!(
                        component = "CommitmentLedger",
                        title = %candidate.title,
                        "active commitment unchanged, skipping"
                    );
                }
                continue;
            }

            let due_at = candidate
                .commitment_due
                .as_deref()
                .and_then(|due| parse_due_at(chrono::Utc::now(), due));

            let new_item = NewCommitment {
                character_id: ctx.character_id.to_string(),
                user_id: ctx.user_id.to_string(),
                title: candidate.title.clone(),
                description: candidate.content.clone(),
                status: CommitmentStatus::Active,
                due_at,
                due_label: candidate.commitment_due.clone(),
            };

            let id = store
                .insert_commitment(&new_item)
                .await
                .map_err(CognitionError::MemoryPort)?;

            debug!(
                component = "CommitmentLedger",
                commitment_id = id,
                title = %candidate.title,
                "inserted active commitment (ledger-first)"
            );
            active.push(inserted_commitment(id, ctx, candidate));
            inserted.push(id);
        }

        Ok(inserted)
    }

    /// Arbitrate non-commitment candidates; write commitments ledger-first.
    pub async fn arbitrate_apply_and_sync(
        store: &dyn MemoryPort,
        candidates: &[MemoryCandidate],
        arbiter_ctx: &ArbiterContext<'_>,
        sync_ctx: &CommitmentSyncContext<'_>,
    ) -> Result<(Vec<AppliedDecision>, Vec<i64>), CognitionError> {
        let (commitment_candidates, other_candidates): (Vec<_>, Vec<_>) = candidates
            .iter()
            .cloned()
            .partition(|c| c.kind == MemoryKind::Commitment);

        let commitment_ids =
            Self::apply_commitment_candidates(store, sync_ctx, &commitment_candidates).await?;

        let applied: Vec<AppliedDecision> = if other_candidates.is_empty() {
            Vec::new()
        } else {
            MemoryArbiter::arbitrate_and_apply(store, &other_candidates, arbiter_ctx).await?
        };

        Ok((applied, commitment_ids))
    }

    /// List active commitments for prompt injection (independent of vector recall).
    ///
    /// Takes `&dyn MemoryPort` (rather than the concrete `MemoryStore`, like
    /// this ledger's other associated functions) because its only caller
    /// outside this module, `ene-mind`'s recall runner, holds its
    /// store handle behind that abstraction.
    pub async fn list_active(
        store: &dyn MemoryPort,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Commitment>, CognitionError> {
        store
            .list_active_commitments(character_id, user_id, limit)
            .await
            .map_err(CognitionError::MemoryPort)
    }

    /// Map active commitments to lightweight prompt DTOs.
    pub fn active_prompt_candidates(commitments: &[Commitment]) -> Vec<ActiveCommitmentPrompt> {
        commitments
            .iter()
            .filter_map(|c| {
                let id = c.id?;
                Some(ActiveCommitmentPrompt {
                    id,
                    title: c.title.clone(),
                    description: c.description.clone(),
                    due_label: c.due_label.clone(),
                    due_at: c.due_at,
                })
            })
            .collect()
    }

    /// Mark a commitment as done.
    pub async fn complete(store: &dyn MemoryPort, id: i64) -> Result<bool, CognitionError> {
        store
            .complete_commitment(id)
            .await
            .map_err(CognitionError::MemoryPort)
    }

    /// Mark a commitment as cancelled.
    pub async fn cancel(store: &dyn MemoryPort, id: i64) -> Result<bool, CognitionError> {
        store
            .cancel_commitment(id)
            .await
            .map_err(CognitionError::MemoryPort)
    }

    /// Mark overdue active commitments as stale.
    pub async fn mark_stale_overdue(
        store: &dyn MemoryPort,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<usize, CognitionError> {
        store
            .mark_stale_commitments(now)
            .await
            .map_err(CognitionError::MemoryPort)
    }

    /// Cancel every active commitment whose title matches `title`.
    ///
    /// Matching uses the shared [`TitleMatcher`], so a rephrased cancellation
    /// ("資料作成をやめる") reaches the commitment registered under a synonymous
    /// title ("資料をまとめる") instead of silently missing it.
    async fn cancel_matching(
        store: &dyn MemoryPort,
        matcher: &mut TitleMatcher<'_>,
        title: &str,
        active: &mut Vec<Commitment>,
    ) -> Result<(), CognitionError> {
        let Some(idx) = Self::find_best_match(matcher, title, active).await else {
            return Ok(());
        };
        let Some(id) = active[idx].id else {
            return Ok(());
        };

        store
            .cancel_commitment(id)
            .await
            .map_err(CognitionError::MemoryPort)?;
        debug!(
            component = "CommitmentLedger",
            commitment_id = id,
            title = %active[idx].title,
            requested = %title,
            "cancelled active commitment"
        );
        // No longer active, so it must not match a later candidate in this batch.
        active.remove(idx);
        Ok(())
    }

    /// Index of the active commitment that best matches `title`, or `None`.
    ///
    /// Shared by the supersede and cancel paths so both resolve a candidate to
    /// **one** commitment. Cancelling every row above the similarity threshold
    /// would be actively wrong now that matching is fuzzy: "資料作成はやめる"
    /// scores highly against "資料をまとめる" and "資料のレビュー" alike, so a
    /// single retraction would silently retire unrelated promises. Superseding
    /// already picks the single closest row; cancellation — which is far harder
    /// to notice and undo — must not be looser than that.
    ///
    /// The pass pins one match mode and redoes the whole search under the exact
    /// fallback if embedding degrades partway, so one scan never mixes
    /// cosine-similarity and exact-equality decisions.
    async fn find_best_match(
        matcher: &mut TitleMatcher<'_>,
        title: &str,
        active: &[Commitment],
    ) -> Option<usize> {
        let titles = commitment_titles(active);
        matcher
            .prefetch(std::iter::once(title).chain(titles.iter().copied()))
            .await;

        let mode = matcher.match_mode();
        let best = matcher.best_match_with(mode, title, &titles).await;
        if mode == TitleMatchMode::Embedding && matcher.embedding_degraded() {
            return matcher
                .best_match_with(TitleMatchMode::Exact, title, &titles)
                .await;
        }
        best
    }
}
/// List active commitments for title matching, warning if the cap is hit.
async fn list_active_for_match(
    store: &dyn MemoryPort,
    ctx: &CommitmentSyncContext<'_>,
) -> Result<Vec<Commitment>, CognitionError> {
    let limit = ctx.active_match_limit.max(1);
    let active = store
        .list_active_commitments(ctx.character_id, Some(ctx.user_id), limit)
        .await
        .map_err(CognitionError::MemoryPort)?;

    if active.len() == limit {
        warn!(
            component = "CommitmentLedger",
            limit,
            "list_active_commitments returned exactly the limit; results may be truncated — \
             raise mind.memory_limits.commitment_active_match_limit \
             (or ENE_MIND__MEMORY_LIMITS__COMMITMENT_ACTIVE_MATCH_LIMIT) if matching misses \
             active commitments"
        );
    }

    Ok(active)
}

/// Titles of `active`, in order, for [`TitleMatcher`]'s slice-based API.
fn commitment_titles(active: &[Commitment]) -> Vec<&str> {
    active.iter().map(|c| c.title.as_str()).collect()
}

/// The row a just-inserted candidate becomes, for mirroring into the in-batch
/// active set.
///
/// Only the fields title matching and supersede comparison read are meaningful;
/// timestamps are approximated with "now" because nothing in the batch reads
/// them. The authoritative row is the one the store wrote.
fn inserted_commitment(
    id: i64,
    ctx: &CommitmentSyncContext<'_>,
    candidate: &MemoryCandidate,
) -> Commitment {
    let now = chrono::Utc::now();
    let due_at = candidate
        .commitment_due
        .as_deref()
        .and_then(|due| parse_due_at(chrono::Utc::now(), due));

    Commitment {
        id: Some(id),
        character_id: ctx.character_id.to_string(),
        user_id: ctx.user_id.to_string(),
        title: candidate.title.clone(),
        description: candidate.content.clone(),
        status: CommitmentStatus::Active,
        due_at,
        due_label: candidate.commitment_due.clone(),
        created_at: now,
        updated_at: now,
        completed_at: None,
    }
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity; deprecated API retained for migration"
)]
mod tests {
    use super::*;
    use crate::memory_writer::candidate::{MemoryCandidate, TurnInput};
    use crate::memory_writer::{ArbiterContext, ArbiterOptions, CandidateProvenance};
    use ene_ai::{EmbeddingKind, EmbeddingProvider};
    use ene_store::MemoryStore;

    fn sync_ctx<'a>() -> CommitmentSyncContext<'a> {
        CommitmentSyncContext {
            character_id: "ene",
            user_id: "user1",
            ..CommitmentSyncContext::default()
        }
    }

    /// Embedder that maps a title to a unit vector keyed by the first topic
    /// keyword it contains, so titles sharing a keyword are near-identical while
    /// unrelated titles are orthogonal. Titles with no known keyword fall back to
    /// a shared neutral vector. Deterministic and dependency-free.
    struct KeywordEmbedder;

    fn keyword_vector(text: &str) -> Vec<f32> {
        let lowered = text.to_lowercase();
        if lowered.contains("design") {
            vec![1.0, 0.0, 0.0]
        } else if lowered.contains("meeting") {
            vec![0.0, 1.0, 0.0]
        } else if lowered.contains("groceries") {
            vec![0.0, 0.0, 1.0]
        } else {
            vec![0.5, 0.5, 0.0]
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for KeywordEmbedder {
        async fn embed_batch(
            &self,
            items: &[(&str, EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            Ok(items.iter().map(|(text, _)| keyword_vector(text)).collect())
        }

        fn dimensions(&self) -> usize {
            3
        }

        fn model_name(&self) -> &'static str {
            "keyword-test"
        }
    }

    /// A sync context wired to the keyword embedder with a fuzzy threshold.
    fn fuzzy_sync_ctx(embedder: &dyn EmbeddingProvider) -> CommitmentSyncContext<'_> {
        CommitmentSyncContext {
            character_id: "ene",
            user_id: "user1",
            embedder: Some(embedder),
            title_similarity_threshold: 0.8,
            ..CommitmentSyncContext::default()
        }
    }

    fn commitment_candidate_titled(title: &str, content: &str) -> MemoryCandidate {
        MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: title.to_string(),
            content: content.to_string(),
            source_quote: content.to_string(),
            confidence: 0.9,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: None,
            tags: Vec::new(),
        }
    }

    fn commitment_candidate(confidence: f32) -> MemoryCandidate {
        MemoryCandidate {
            kind: MemoryKind::Commitment,
            title: "discuss design".to_string(),
            content: "Next time, let's discuss the design".to_string(),
            source_quote: "Next time, let's discuss the design".to_string(),
            confidence,
            should_persist: true,
            deletion_target_key: None,
            commitment_due: Some("Next time".to_string()),
            tags: Vec::new(),
        }
    }

    #[tokio::test]
    async fn apply_creates_active_commitment_ledger_first() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let ids = CommitmentLedger::apply_commitment_candidates(
            &store,
            &sync_ctx(),
            &[commitment_candidate(0.9)],
        )
        .await
        .unwrap();
        assert_eq!(ids.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, CommitmentStatus::Active);
        assert_eq!(active[0].due_label.as_deref(), Some("Next time"));

        let prompts = CommitmentLedger::active_prompt_candidates(&active);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].title, "discuss design");
    }

    #[tokio::test]
    async fn apply_parses_due_label_into_due_at() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut candidate = commitment_candidate(0.9);
        candidate.commitment_due = Some("tomorrow at 15:00".to_string());
        let ids = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[candidate])
            .await
            .unwrap();

        let row = store.get_commitment(ids[0]).await.unwrap().unwrap();
        let due_at = row.due_at.expect("parseable due label must set due_at");
        let now = chrono::Utc::now();
        assert!(due_at > now, "due must be in the future, got {due_at}");
        assert!(
            due_at < now + chrono::Duration::days(2),
            "due must be within two days, got {due_at}"
        );
        assert_eq!(row.due_label.as_deref(), Some("tomorrow at 15:00"));
    }

    #[tokio::test]
    async fn apply_keeps_due_at_none_for_unparseable_label() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut candidate = commitment_candidate(0.9);
        candidate.commitment_due = Some("sometime later".to_string());
        let ids = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[candidate])
            .await
            .unwrap();

        let row = store.get_commitment(ids[0]).await.unwrap().unwrap();
        assert!(row.due_at.is_none());
        assert_eq!(row.due_label.as_deref(), Some("sometime later"));
    }

    #[tokio::test]
    async fn apply_skips_duplicate_title() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let candidates = [commitment_candidate(0.9)];
        let ids1 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert_eq!(ids1.len(), 1);

        let ids2 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert!(ids2.is_empty());

        let active = CommitmentLedger::list_active(&store, "ene", None, 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
    }

    #[tokio::test]
    async fn apply_supersedes_existing_with_changed_content() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let candidates = [commitment_candidate(0.9)];
        let ids1 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &candidates)
            .await
            .unwrap();
        assert_eq!(ids1.len(), 1);

        // Same title, different content and due_label → supersede
        let mut updated = commitment_candidate(0.9);
        updated.content = "Let's discuss the UI design instead".to_string();
        updated.commitment_due = Some("Tomorrow".to_string());

        let ids2 = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[updated])
            .await
            .unwrap();
        assert_eq!(ids2.len(), 1);
        assert_eq!(ids2[0], ids1[0], "superseded row should keep the same id");

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].description, "Let's discuss the UI design instead");
        assert_eq!(active[0].due_label.as_deref(), Some("Tomorrow"));
        assert!(
            active[0].due_at.is_some(),
            "supersede must re-parse the new due label"
        );
    }

    #[tokio::test]
    async fn apply_supersedes_existing_on_due_date_change_only() {
        // Rescheduling the same commitment — same title and content, only
        // the due label changes — must update the existing ledger row, not
        // register a second valid commitment that both survive as active.
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let ids1 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &sync_ctx(),
            &[commitment_candidate(0.9)],
        )
        .await
        .unwrap();
        assert_eq!(ids1.len(), 1);

        let mut rescheduled = commitment_candidate(0.9);
        rescheduled.commitment_due = Some("Next week".to_string());

        let ids2 =
            CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[rescheduled])
                .await
                .unwrap();
        assert_eq!(ids2.len(), 1);
        assert_eq!(
            ids2[0], ids1[0],
            "rescheduling must keep the same commitment id"
        );

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(
            active.len(),
            1,
            "rescheduling must not duplicate the commitment"
        );
        assert_eq!(active[0].due_label.as_deref(), Some("Next week"));
        assert!(
            active[0].due_at.is_some(),
            "reschedule must re-parse the new due label"
        );
    }

    #[tokio::test]
    async fn apply_ignores_non_commitment_candidates() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let mut candidate = commitment_candidate(0.9);
        candidate.kind = MemoryKind::Semantic;
        let ids = CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[candidate])
            .await
            .unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn apply_deletion_cancels_matching_title() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        CommitmentLedger::apply_commitment_candidates(
            &store,
            &sync_ctx(),
            &[commitment_candidate(0.9)],
        )
        .await
        .unwrap();

        let mut deletion = commitment_candidate(0.9);
        deletion.should_persist = false;
        CommitmentLedger::apply_commitment_candidates(&store, &sync_ctx(), &[deletion])
            .await
            .unwrap();

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert!(active.is_empty());
    }

    #[tokio::test]
    async fn arbitrate_apply_and_sync_end_to_end_ledger_first() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let turn = TurnInput {
            user_message: "Next time, let's discuss the design",
            assistant_message: None,
            tool_results: &[],
        };
        let arbiter_ctx = ArbiterContext {
            turn,
            character_id: "ene",
            user_id: "user1",
            source_ref: Some("session-1"),
            source_turn: None,
            provenance: CandidateProvenance::Deterministic,
            options: ArbiterOptions {
                min_confidence: 0.4,
                ..ArbiterOptions::default()
            },
            semantic_matches: Default::default(),
            affect_valence: 0.0,
            embedder: None,
        };
        let candidate = commitment_candidate(0.9);

        let (applied, commitment_ids) = CommitmentLedger::arbitrate_apply_and_sync(
            &store,
            std::slice::from_ref(&candidate),
            &arbiter_ctx,
            &sync_ctx(),
        )
        .await
        .unwrap();

        // Commitment candidates bypass typed arbiter persist.
        assert!(applied.is_empty());
        assert_eq!(commitment_ids.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "discuss design");
    }

    #[tokio::test]
    async fn complete_and_cancel_lifecycle() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let id = store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                title: "follow up".to_string(),
                description: "Talk about project X".to_string(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();

        assert!(CommitmentLedger::complete(&store, id).await.unwrap());
        let done = store.get_commitment(id).await.unwrap().unwrap();
        assert_eq!(done.status, CommitmentStatus::Done);
        assert!(done.completed_at.is_some());

        let id2 = store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: String::new(),
                title: "other".to_string(),
                description: "other task".to_string(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();
        assert!(CommitmentLedger::cancel(&store, id2).await.unwrap());
        let cancelled = store.get_commitment(id2).await.unwrap().unwrap();
        assert_eq!(cancelled.status, CommitmentStatus::Cancelled);
    }

    #[tokio::test]
    async fn apply_supersedes_rephrased_title_via_embedding() {
        // A rephrased commitment ("write design doc" vs "design review")
        // must supersede the existing row rather than register a duplicate,
        // because the titles are embedding-similar.
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = KeywordEmbedder;
        let ctx = fuzzy_sync_ctx(&embedder);

        let ids1 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "design review",
                "Review the design",
            )],
        )
        .await
        .unwrap();
        assert_eq!(ids1.len(), 1);

        // Rephrased title, same topic, different content → supersede, not insert.
        let ids2 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "write design doc",
                "Write up the design document",
            )],
        )
        .await
        .unwrap();
        assert_eq!(ids2.len(), 1);
        assert_eq!(ids2[0], ids1[0], "rephrased title should supersede same id");

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1, "rephrased commitment must not duplicate");
        assert_eq!(active[0].description, "Write up the design document");
    }

    #[tokio::test]
    async fn apply_keeps_unrelated_titles_separate_via_embedding() {
        // Embedding matching must not over-merge — unrelated titles stay
        // separate commitments.
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = KeywordEmbedder;
        let ctx = fuzzy_sync_ctx(&embedder);

        CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "design review",
                "Review the design",
            )],
        )
        .await
        .unwrap();

        let ids2 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "buy groceries",
                "Pick up groceries",
            )],
        )
        .await
        .unwrap();
        assert_eq!(ids2.len(), 1);

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 2, "unrelated titles must stay separate");
    }

    #[tokio::test]
    async fn apply_deletion_cancels_rephrased_title_via_embedding() {
        // A rephrased cancellation reaches the synonymously-titled row.
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = KeywordEmbedder;
        let ctx = fuzzy_sync_ctx(&embedder);

        CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "design review",
                "Review the design",
            )],
        )
        .await
        .unwrap();

        let mut deletion = commitment_candidate_titled("write design doc", "no longer needed");
        deletion.should_persist = false;
        CommitmentLedger::apply_commitment_candidates(&store, &ctx, &[deletion])
            .await
            .unwrap();

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert!(
            active.is_empty(),
            "rephrased cancellation must cancel the row"
        );
    }

    #[tokio::test]
    async fn embedding_failure_falls_back_to_exact_matching() {
        // If the embedder errors, the ledger degrades to exact matching
        // rather than failing the write or double-registering exact duplicates.
        struct FailingEmbedder;

        #[async_trait::async_trait]
        impl EmbeddingProvider for FailingEmbedder {
            async fn embed_batch(
                &self,
                _items: &[(&str, EmbeddingKind)],
            ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
                Err(ene_ai::EmbeddingError::Provider("simulated failure".into()))
            }

            fn dimensions(&self) -> usize {
                3
            }

            fn model_name(&self) -> &'static str {
                "failing-test"
            }
        }

        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = FailingEmbedder;
        let ctx = fuzzy_sync_ctx(&embedder);

        let ids1 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "design review",
                "Review the design",
            )],
        )
        .await
        .unwrap();
        assert_eq!(ids1.len(), 1);

        // Exact duplicate still dedups via the fallback path.
        let ids2 = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "design review",
                "Review the design",
            )],
        )
        .await
        .unwrap();
        assert!(
            ids2.is_empty(),
            "exact duplicate must still dedup on fallback"
        );

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
    }

    /// Matcher mechanics are covered in `crate::title_match`; this pins the
    /// ledger's own responsibility — when embedding fails partway through a
    /// scan, the whole search is redone under exact matching, so a title that
    /// differs only in formatting still supersedes instead of inserting a
    /// duplicate.
    #[tokio::test]
    async fn embedding_failure_mid_scan_still_dedups_via_exact_redo() {
        struct FailAfterFirstEmbedder {
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait::async_trait]
        impl EmbeddingProvider for FailAfterFirstEmbedder {
            async fn embed_batch(
                &self,
                items: &[(&str, ene_ai::EmbeddingKind)],
            ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
                let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n >= 1 {
                    return Err(ene_ai::EmbeddingError::Provider(
                        "forced mid-scan failure".to_string(),
                    ));
                }
                Ok(items.iter().map(|_| vec![1.0, 0.0, 0.0]).collect())
            }

            fn dimensions(&self) -> usize {
                3
            }

            fn model_name(&self) -> &'static str {
                "fail-after-first"
            }
        }

        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = FailAfterFirstEmbedder {
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let ctx = CommitmentSyncContext {
            character_id: "ene",
            user_id: "user1",
            embedder: Some(&embedder),
            title_similarity_threshold: 0.8,
            ..CommitmentSyncContext::default()
        };

        CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "Design Review",
                "review the design",
            )],
        )
        .await
        .unwrap();

        // Same subject, different formatting. The embedding path is degraded by
        // now, so only the exact redo can recognize it.
        CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "  design　review ",
                "review the design once more",
            )],
        )
        .await
        .unwrap();

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(
            active.len(),
            1,
            "a degraded scan must still supersede, not insert a duplicate"
        );
        assert_eq!(active[0].description, "review the design once more");
    }

    /// A retraction must retire exactly the commitment it names. Under fuzzy
    /// matching several active promises can clear the similarity threshold at
    /// once ("design review", "design doc", "design sync" all embed alike), and
    /// cancelling every one of them would silently retire promises the user
    /// never withdrew.
    #[tokio::test]
    async fn cancellation_retires_only_the_closest_commitment() {
        /// Graded so "design review" is nearer the retraction than "design doc"
        /// while both clear the 0.8 threshold — the ambiguous case the single
        /// best match has to resolve.
        struct GradedEmbedder;

        #[async_trait::async_trait]
        impl EmbeddingProvider for GradedEmbedder {
            async fn embed_batch(
                &self,
                items: &[(&str, EmbeddingKind)],
            ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
                Ok(items
                    .iter()
                    .map(|(text, _)| {
                        let lowered = text.to_lowercase();
                        if lowered.contains("review") {
                            vec![1.0, 0.0, 0.0]
                        } else if lowered.contains("design") {
                            vec![0.9, 0.436, 0.0]
                        } else {
                            vec![0.0, 0.0, 1.0]
                        }
                    })
                    .collect())
            }

            fn dimensions(&self) -> usize {
                3
            }

            fn model_name(&self) -> &'static str {
                "graded-test"
            }
        }

        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedder = GradedEmbedder;
        let ctx = fuzzy_sync_ctx(&embedder);

        for title in ["design review", "design doc", "buy groceries"] {
            CommitmentLedger::apply_commitment_candidates(
                &store,
                &sync_ctx(),
                &[commitment_candidate_titled(title, title)],
            )
            .await
            .unwrap();
        }
        assert_eq!(
            CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
                .await
                .unwrap()
                .len(),
            3
        );

        let mut retract = commitment_candidate_titled("design review", "never mind");
        retract.should_persist = false;
        CommitmentLedger::apply_commitment_candidates(&store, &ctx, &[retract])
            .await
            .unwrap();

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        let titles: Vec<&str> = active.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(
            active.len(),
            2,
            "exactly one commitment should be cancelled, got {titles:?}"
        );
        assert!(
            titles.contains(&"buy groceries"),
            "an unrelated commitment must survive, got {titles:?}"
        );
        assert!(
            titles.contains(&"design doc"),
            "a merely similar commitment must survive, got {titles:?}"
        );
    }

    /// `list_active_commitments` returns dated rows before undated ones. With
    /// `active_match_limit = 1`, only the dated row is loaded for matching, so
    /// re-applying the truncated undated title inserts a duplicate instead of
    /// superseding — proving the cap is live.
    #[tokio::test]
    async fn active_match_limit_truncates_older_undated_rows() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();

        let undated_id = store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                title: "undated promise".to_string(),
                description: "original undated".to_string(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();
        store
            .insert_commitment(&NewCommitment {
                character_id: "ene".to_string(),
                user_id: "user1".to_string(),
                title: "dated promise".to_string(),
                description: "has a due date".to_string(),
                status: CommitmentStatus::Active,
                due_at: Some(chrono::Utc::now() + chrono::Duration::days(1)),
                due_label: Some("tomorrow".to_string()),
            })
            .await
            .unwrap();

        let listed = store
            .list_active_commitments("ene", Some("user1"), 1)
            .await
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].title, "dated promise",
            "dated rows must sort ahead of undated so limit=1 drops the undated title"
        );

        let ctx = CommitmentSyncContext {
            character_id: "ene",
            user_id: "user1",
            active_match_limit: 1,
            ..CommitmentSyncContext::default()
        };
        let ids = CommitmentLedger::apply_commitment_candidates(
            &store,
            &ctx,
            &[commitment_candidate_titled(
                "undated promise",
                "should insert because the undated row was truncated",
            )],
        )
        .await
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert_ne!(
            ids[0], undated_id,
            "truncated match set must not supersede the missing undated row"
        );

        let active = CommitmentLedger::list_active(&store, "ene", Some("user1"), 10)
            .await
            .unwrap();
        assert_eq!(
            active.len(),
            3,
            "truncated limit must allow a duplicate insert of the omitted title"
        );
    }
}
