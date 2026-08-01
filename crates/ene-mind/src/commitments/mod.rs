//! Companion Commitment Ledger — promises, tasks, and follow-ups.
//!
//! The `commitments` table is the sole source of truth for commitment lifecycle
//! and prompt injection (#124). Typed `MemoryKind::Commitment` rows are optional
//! references (`typed_memories.commitment_id`) and are no longer dual-written
//! from arbiter persist → sync.
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
use std::collections::HashMap;

use ene_ai::{EmbeddingKind, EmbeddingProvider, cosine_similarity};
use ene_core::{ActiveCommitmentPrompt, Commitment, MemoryPort};
use ene_core::{CommitmentStatus, MemoryKind, NewCommitment};
use tracing::{debug, warn};

use crate::error::CognitionError;
use crate::memory_writer::candidate::MemoryCandidate;
use crate::memory_writer::{AppliedDecision, ArbiterContext, MemoryArbiter};

/// Maximum active commitments to consider for title-matching dedup / deletion.
/// Set to 4096 — well above any plausible concurrent active‑commitment count.
/// A warning is emitted when the cap is hit so operators can tune if needed.
const MAX_ACTIVE_MATCH_CHECK: usize = 4096;

/// Companion Commitment Ledger: promises, tasks, follow-ups.
#[derive(Debug, Default)]
pub struct CommitmentLedger;

/// Context required when writing ledger rows from commitment candidates.
///
/// Carries the optional embedding provider used to match commitments by title
/// *similarity* rather than exact string equality (#387). When `embedder` is
/// `None`, matching degrades to the deterministic exact-title fallback so the
/// ledger keeps working without an embedding model.
#[derive(Clone, Default)]
pub struct CommitmentSyncContext<'a> {
    /// Character identifier.
    pub character_id: &'a str,
    /// User identifier (may be empty).
    pub user_id: &'a str,
    /// Embedding provider for fuzzy title matching (#387). `None` falls back to
    /// exact normalized-title equality.
    pub embedder: Option<&'a dyn EmbeddingProvider>,
    /// Minimum title-embedding cosine similarity for two commitments to be
    /// treated as the same one (#387). Only consulted on the embedding path.
    pub title_similarity_threshold: f32,
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
            .finish()
    }
}

impl CommitmentLedger {
    /// Persist commitment candidates to the ledger first (sole `SoT`).
    ///
    /// Does **not** insert typed `MemoryKind::Commitment` bodies. Deletion-style
    /// candidates (`should_persist = false`) cancel matching active ledger rows
    /// by title. Matching is by title-embedding similarity when an embedder is
    /// configured (#387), falling back to exact normalized-title equality
    /// otherwise.
    pub async fn apply_commitment_candidates(
        store: &dyn MemoryPort,
        ctx: &CommitmentSyncContext<'_>,
        candidates: &[MemoryCandidate],
    ) -> Result<Vec<i64>, CognitionError> {
        let mut inserted = Vec::new();

        // The matcher caches title embeddings across iterations, so re-listing
        // active commitments per candidate (needed so an insert/supersede in one
        // iteration is visible to the next) does not re-embed repeated titles.
        let mut matcher = TitleMatcher::new(ctx.embedder, ctx.title_similarity_threshold);

        for candidate in candidates {
            if candidate.kind != MemoryKind::Commitment {
                continue;
            }

            if !candidate.should_persist {
                Self::cancel_matching(store, ctx, &mut matcher, &candidate.title).await?;
                continue;
            }

            let active = list_active_for_match(store, ctx).await?;
            if let Some(existing) = matcher
                .best_match(&candidate.title, &active)
                .await
                .map(|idx| &active[idx])
            {
                // Same commitment exists — supersede (update description/due_label) if content differs.
                let content_changed = existing.description != candidate.content;
                let due_changed =
                    existing.due_label.as_deref() != candidate.commitment_due.as_deref();
                if content_changed || due_changed {
                    if let Some(id) = existing.id {
                        store
                            .supersede_commitment(
                                id,
                                &candidate.content,
                                candidate.commitment_due.as_deref(),
                            )
                            .await
                            .map_err(CognitionError::MemoryPort)?;
                        debug!(
                            component = "CommitmentLedger",
                            commitment_id = id,
                            title = %candidate.title,
                            "superseded active commitment (description/due_label updated)"
                        );
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

            let new_item = NewCommitment {
                character_id: ctx.character_id.to_string(),
                user_id: ctx.user_id.to_string(),
                title: candidate.title.clone(),
                description: candidate.content.clone(),
                status: CommitmentStatus::Active,
                due_at: None,
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
            inserted.push(id);
        }

        Ok(inserted)
    }

    /// Arbitrate non-commitment candidates; write commitments ledger-first.
    ///
    /// Replaces the former dual-write path (`arbitrate` → typed persist →
    /// `sync_from_applied_decisions`).
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
    /// outside this module, `ene-mind`'s recall runner (#270), holds its
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

    /// Cancel every active commitment whose title matches `title` (#387).
    ///
    /// Matching uses the shared [`TitleMatcher`], so a rephrased cancellation
    /// ("資料作成をやめる") reaches the commitment registered under a synonymous
    /// title ("資料をまとめる") instead of silently missing it.
    async fn cancel_matching(
        store: &dyn MemoryPort,
        ctx: &CommitmentSyncContext<'_>,
        matcher: &mut TitleMatcher<'_>,
        title: &str,
    ) -> Result<(), CognitionError> {
        let active = list_active_for_match(store, ctx).await?;

        let mut matching: Vec<(i64, String)> = Vec::new();
        for row in &active {
            let Some(id) = row.id else {
                continue;
            };
            if matcher.is_match(&row.title, title).await {
                matching.push((id, row.title.clone()));
            }
        }

        if matching.len() > 1 {
            warn!(
                component = "CommitmentLedger",
                title = %title,
                count = matching.len(),
                ids = ?matching.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                "cancel_matching matched multiple active commitments; ambiguity may cause unintended cancellation"
            );
        }

        // Accumulate errors across all rows so a single failure
        // does not leave the caller in a partially-cancelled state.
        let mut errors: Vec<String> = Vec::new();
        for (id, _) in &matching {
            if let Err(e) = store.cancel_commitment(*id).await {
                errors.push(format!("commitment {id}: {e}"));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(CognitionError::Aggregate(errors.join("; ")))
        }
    }
}

/// Decides whether two commitment titles refer to the same commitment (#387).
///
/// When an [`EmbeddingProvider`] is configured, titles are compared by the
/// cosine similarity of their embeddings and a pair matches once the similarity
/// reaches the configured threshold — this is what lets "資料をまとめる" and
/// "資料作成" collapse into one commitment. Without an embedder (or if embedding
/// ever fails) the matcher degrades to the deterministic exact fallback (trim +
/// lowercase equality), so the ledger never silently stops deduplicating.
///
/// Embeddings are cached by title for the lifetime of the matcher, so re-listing
/// active commitments per candidate does not re-embed repeated titles.
struct TitleMatcher<'a> {
    /// Embedding provider; `None` selects the exact-match fallback.
    embedder: Option<&'a dyn EmbeddingProvider>,
    /// Cosine-similarity cutoff for a match (embedding path only).
    threshold: f32,
    /// Embeddings computed so far, keyed by exact title string.
    cache: HashMap<String, Vec<f32>>,
    /// Set once embedding fails, after which matching falls back to exact.
    disabled: bool,
}

impl<'a> TitleMatcher<'a> {
    /// Create a matcher. Embeddings are computed lazily and cached.
    fn new(embedder: Option<&'a dyn EmbeddingProvider>, threshold: f32) -> Self {
        Self {
            embedder,
            threshold,
            cache: HashMap::new(),
            disabled: false,
        }
    }

    /// Embed a single title, caching the result. Returns `None` when no embedder
    /// is available, embedding has been disabled by a prior failure, or the
    /// provider rejects this title.
    async fn embed(&mut self, title: &str) -> Option<Vec<f32>> {
        if !self.use_embedding() {
            return None;
        }
        if let Some(embedding) = self.cache.get(title) {
            return Some(embedding.clone());
        }
        let embedder = self.embedder?;
        match embedder
            .embed_batch(&[(title, EmbeddingKind::Summary)])
            .await
        {
            Ok(mut vectors) if vectors.len() == 1 => {
                let embedding = vectors.swap_remove(0);
                self.cache.insert(title.to_string(), embedding.clone());
                Some(embedding)
            }
            Ok(vectors) => {
                warn!(
                    component = "CommitmentLedger",
                    returned = vectors.len(),
                    "commitment title embedding batch size mismatch; falling back to exact title matching"
                );
                self.disabled = true;
                None
            }
            Err(error) => {
                warn!(
                    component = "CommitmentLedger",
                    error = %error,
                    "commitment title embedding failed; falling back to exact title matching"
                );
                self.disabled = true;
                None
            }
        }
    }

    /// Cosine similarity between two titles' embeddings, embedding on demand.
    async fn similarity(&mut self, left: &str, right: &str) -> Option<f32> {
        let left_embedding = self.embed(left).await?;
        let right_embedding = self.embed(right).await?;
        Some(cosine_similarity(&left_embedding, &right_embedding))
    }

    /// Whether the embedding path is currently usable.
    fn use_embedding(&self) -> bool {
        self.embedder.is_some() && !self.disabled
    }

    /// Whether `candidate_title` refers to the same commitment as `active_title`.
    ///
    /// Uses embedding similarity when available; if embedding is unavailable or
    /// fails mid-call, falls back to exact normalized-title equality.
    async fn is_match(&mut self, active_title: &str, candidate_title: &str) -> bool {
        if self.use_embedding()
            && let Some(similarity) = self.similarity(active_title, candidate_title).await
        {
            return similarity >= self.threshold;
        }
        normalize_title(active_title) == normalize_title(candidate_title)
    }

    /// The index of the active commitment whose title best matches
    /// `candidate_title`, provided the match reaches the threshold.
    ///
    /// On the exact path this is the first normalized-title equality; on the
    /// embedding path it is the highest-similarity row at or above the threshold.
    /// If embedding fails mid-call, the search falls back to exact matching so a
    /// transient model error never lets a duplicate slip through.
    async fn best_match(&mut self, candidate_title: &str, active: &[Commitment]) -> Option<usize> {
        if self.use_embedding() {
            let mut best: Option<(f32, usize)> = None;
            for (idx, commitment) in active.iter().enumerate() {
                let Some(similarity) = self.similarity(&commitment.title, candidate_title).await
                else {
                    continue;
                };
                if similarity >= self.threshold
                    && best.is_none_or(|(best_similarity, _)| similarity > best_similarity)
                {
                    best = Some((similarity, idx));
                }
            }
            // A match wins; a clean (non-disabled) miss returns `None`. Only an
            // embedding failure (now disabled) falls through to exact matching.
            if best.is_some() || !self.disabled {
                return best.map(|(_, idx)| idx);
            }
        }

        active.iter().position(|commitment| {
            normalize_title(&commitment.title) == normalize_title(candidate_title)
        })
    }
}

/// List active commitments for title matching, warning if the cap is hit.
async fn list_active_for_match(
    store: &dyn MemoryPort,
    ctx: &CommitmentSyncContext<'_>,
) -> Result<Vec<Commitment>, CognitionError> {
    let active = store
        .list_active_commitments(ctx.character_id, Some(ctx.user_id), MAX_ACTIVE_MATCH_CHECK)
        .await
        .map_err(CognitionError::MemoryPort)?;

    if active.len() == MAX_ACTIVE_MATCH_CHECK {
        warn!(
            component = "CommitmentLedger",
            limit = MAX_ACTIVE_MATCH_CHECK,
            "list_active_commitments returned exactly the limit; results may be truncated"
        );
    }

    Ok(active)
}

fn normalize_title(title: &str) -> String {
    title.trim().to_lowercase()
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

        // Insert the initial commitment
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
        // #387: a rephrased commitment ("write design doc" vs "design review")
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
        // #387: embedding matching must not over-merge — unrelated titles stay
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
        // #387: a rephrased cancellation reaches the synonymously-titled row.
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
        // #387: if the embedder errors, the ledger degrades to exact matching
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

    #[tokio::test]
    async fn title_matcher_exact_mode_matches_normalized_titles() {
        let mut matcher = TitleMatcher::new(None, 0.8);
        assert!(matcher.is_match("Design Review", "  design review ").await);
        assert!(!matcher.is_match("design review", "write design doc").await);
        assert!(matcher.embed("anything").await.is_none());
    }

    #[tokio::test]
    async fn title_matcher_embedding_mode_uses_threshold() {
        let embedder = KeywordEmbedder;
        let active = vec![Commitment {
            id: Some(1),
            character_id: "ene".to_string(),
            user_id: "user1".to_string(),
            title: "design review".to_string(),
            description: String::new(),
            status: CommitmentStatus::Active,
            due_at: None,
            due_label: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            completed_at: None,
        }];

        let mut matcher = TitleMatcher::new(Some(&embedder), 0.8);

        assert!(matcher.is_match("design review", "write design doc").await);
        assert!(!matcher.is_match("design review", "buy groceries").await);

        let best = matcher.best_match("write design doc", &active).await;
        assert_eq!(best, Some(0));
        assert!(matcher.best_match("buy groceries", &active).await.is_none());
    }
}
