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

/// Maximum active commitments to consider for title-matching dedup / deletion.
/// Set to 4096 — well above any plausible concurrent active‑commitment count.
/// A warning is emitted when the cap is hit so operators can tune if needed.
const MAX_ACTIVE_MATCH_CHECK: usize = 4096;

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
#[derive(Clone, Default)]
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
    /// configured, falling back to exact normalized-title equality
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
        let mut matcher = TitleMatcher::new(
            ctx.embedder,
            ctx.title_similarity_threshold,
            "CommitmentLedger",
        );

        for candidate in candidates {
            if candidate.kind != MemoryKind::Commitment {
                continue;
            }

            if !candidate.should_persist {
                Self::cancel_matching(store, ctx, &mut matcher, &candidate.title).await?;
                continue;
            }

            let active = list_active_for_match(store, ctx).await?;
            let titles = commitment_titles(&active);
            matcher
                .prefetch(std::iter::once(candidate.title.as_str()).chain(titles.iter().copied()))
                .await;
            let mode = matcher.match_mode();
            let mut best = matcher
                .best_match_with(mode, &candidate.title, &titles)
                .await;
            // A mid-scan embedding failure would leave the first titles compared
            // by similarity and the rest by exact equality; redo the whole scan
            // under the fallback so one pass has one semantics.
            if mode == TitleMatchMode::Embedding && matcher.embedding_degraded() {
                best = matcher
                    .best_match_with(TitleMatchMode::Exact, &candidate.title, &titles)
                    .await;
            }
            if let Some(existing) = best.map(|idx| &active[idx]) {
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
        ctx: &CommitmentSyncContext<'_>,
        matcher: &mut TitleMatcher<'_>,
        title: &str,
    ) -> Result<(), CognitionError> {
        let active = list_active_for_match(store, ctx).await?;

        // Pin semantics for the whole cancel pass; redo under exact if embedding
        // degrades mid-loop so exact and embedding matches never mix.
        let mode = matcher.match_mode();
        let mut matching = collect_title_matches(matcher, mode, title, &active).await;
        if mode == TitleMatchMode::Embedding && matcher.embedding_degraded() {
            matching = collect_title_matches(matcher, TitleMatchMode::Exact, title, &active).await;
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

async fn collect_title_matches(
    matcher: &mut TitleMatcher<'_>,
    mode: TitleMatchMode,
    title: &str,
    active: &[Commitment],
) -> Vec<(i64, String)> {
    let mut matching: Vec<(i64, String)> = Vec::new();
    for row in active {
        let Some(id) = row.id else {
            continue;
        };
        if matcher.is_match_with(mode, &row.title, title).await {
            matching.push((id, row.title.clone()));
        }
    }
    matching
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

/// Titles of `active`, in order, for [`TitleMatcher`]'s slice-based API.
fn commitment_titles(active: &[Commitment]) -> Vec<&str> {
    active.iter().map(|c| c.title.as_str()).collect()
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
}
