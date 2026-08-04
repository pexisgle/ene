//! End-to-end hybrid recall execution for the cognitive runtime.

use chrono::Utc;
use ene_core::MemoryPort;

use super::MemoryRecallCache;
use super::pending::gather_pending_candidates;
use crate::character::is_lorebook_memory_row;
use crate::commitments::CommitmentLedger;
use crate::config::MindConfig;
use crate::error::CognitionError;
use crate::memory_writer::reflection::{apply_reflection_adjustment, load_reflection_memories};
use crate::recall::{
    MemoryDiversifyOptions, MemoryDiversifyPipeline, RecallPlanner, RecallPlannerInput,
    RecallPlannerOptions, RecallResultMapper, RecallTurn, RecalledMemory,
};

/// Input for executing hybrid typed-memory recall.
pub struct ExecuteRecallInput<'a> {
    /// Memory store handle (behind the `MemoryPort` abstraction).
    pub store: &'a dyn MemoryPort,
    /// Character card name.
    pub character_id: &'a str,
    /// User name / id for scoping.
    pub user_id: &'a str,
    /// Current user message.
    pub user_input: &'a str,
    /// Recent conversation turns for planning.
    pub recent_turns: &'a [RecallTurn<'a>],
    /// Query embedding vector.
    pub query_embedding: &'a [f32],
    /// Embedding model name.
    pub embedding_model: &'a str,
    /// Loaded affect state (optional).
    pub affect: Option<&'a ene_core::AffectState>,
    /// L1 recall cache; `None` falls back to L2 for every query.
    pub cache: Option<&'a MemoryRecallCache>,
    /// Session identifier for cache scoping.
    pub session_id: &'a str,
}

/// Execute hybrid typed recall and return plan + recalled memories.
pub async fn execute_hybrid_recall(
    config: &MindConfig,
    input: &ExecuteRecallInput<'_>,
) -> Result<(crate::recall::RecallPlan, Vec<RecalledMemory>), CognitionError> {
    let commitments = match list_active_commitments(input).await {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(
                component = "RecallRunner",
                error = %error,
                "Failed to list active commitments for recall planning"
            );
            vec![]
        }
    };
    let commitment_prompts = CommitmentLedger::active_prompt_candidates(&commitments);

    let planner_input = RecallPlannerInput {
        user_input: input.user_input,
        recent_turns: input.recent_turns,
        scene_summary: None,
        affect: input.affect,
        commitments: &commitment_prompts,
        language: config.resolved_classifier_language(),
        character_id: input.character_id,
        user_id: Some(input.user_id),
    };

    let options = RecallPlannerOptions::from_config(&config.memory);
    let plan = RecallPlanner::plan(&planner_input, &options)?;

    let search_options = RecallPlanner::to_memory_search_options(
        &plan,
        input.query_embedding,
        input.embedding_model,
        Utc::now(),
        &config.memory,
    );

    let mut gathered = match input.cache {
        Some(cache) => {
            cache
                .search(input.store, input.session_id, &search_options)
                .await
        }
        None => input.store.search(&search_options).await,
    }
    .map_err(CognitionError::MemoryPort)?;

    // Unconfirmed candidates deferred to the user-approval queue compete
    // in the same score competition as typed memories. They carry no
    // embedding, so `search` can never gather them; load the live `pending`
    // queue and merge the topic-related ones here. `score_and_rank` below then
    // applies the ordinary min_score floor and result limit, so a pending
    // candidate only surfaces when the conversation touches its topic.
    let pending_limit = config.memory.recall_pending_candidate_limit;
    if pending_limit > 0 {
        gather_pending_candidates(
            input.cache,
            input.store,
            &search_options,
            &mut gathered,
            pending_limit,
        )
        .await;
    }

    // Lorebook rows are card data with a guaranteed-injection path of their
    // own (prompt composition), so embedding-similarity recall must not
    // surface them again inside the memory sections. Filtering before scoring
    // and MMR keeps them from consuming search, score, and diversify slots.
    gathered.retain(|c| !is_lorebook_memory_row(c.item.source, c.item.source_ref.as_deref()));

    let mut scored = ene_rag::score_and_rank(&search_options, gathered);

    // Close the self-reflection feedback loop: reflections are excluded
    // from the search query above (they are a scoring signal, not recall
    // results), so load them separately and apply their boost/penalty to the
    // scored memories. Gated by the reflection `enabled` config.
    apply_reflection_to_scored(config, input, &mut scored).await;

    let diversify_options = MemoryDiversifyOptions::from_config(&config.memory);
    let diversified = MemoryDiversifyPipeline::diversify(scored, &plan, diversify_options);

    let recalled = RecallResultMapper::map(diversified);

    Ok((plan, recalled))
}

/// Load active commitments through the L1 cache when present, else L2.
async fn list_active_commitments(
    input: &ExecuteRecallInput<'_>,
) -> Result<Vec<ene_core::Commitment>, ene_core::MemoryPortError> {
    match input.cache {
        Some(cache) => {
            cache
                .list_active_commitments(input.store, input.character_id, Some(input.user_id), 16)
                .await
        }
        None => {
            input
                .store
                .list_active_commitments(input.character_id, Some(input.user_id), 16)
                .await
        }
    }
}

/// Load reflection memories and apply their boost/penalty to scored recall
/// candidates, closing the self-reflection feedback loop.
///
/// Reflections are a scoring signal, not recall results — the search query
/// already excludes [`ene_core::MemoryKind::Reflection`], so this loads them
/// through a dedicated path and adjusts the hybrid totals in place. The
/// adjustment records a `reflection_multiplier` in each affected breakdown,
/// keeping the explainable score consistent. No-op when the pipeline is
/// disabled or there are no scored candidates.
async fn apply_reflection_to_scored(
    config: &MindConfig,
    input: &ExecuteRecallInput<'_>,
    scored: &mut [ene_core::ScoredMemory],
) {
    let reflection = &config.memory.reflection;
    if !reflection.enabled || scored.is_empty() {
        return;
    }

    let reflections = match input.cache {
        Some(cache) => cache
            .get_reflection_memories(input.store, input.character_id)
            .await
            .unwrap_or_default(),
        None => load_reflection_memories(input.store, input.character_id).await,
    };
    if reflections.is_empty() {
        return;
    }

    apply_reflection_adjustment(
        scored,
        &reflections,
        reflection.success_boost,
        reflection.failure_penalty,
    );

    // Totals changed, so restore the descending-total ordering that
    // `score_and_rank` produced before diversification consumes it.
    scored.sort_by(|a, b| {
        b.breakdown
            .total
            .total_cmp(&a.breakdown.total)
            .then_with(|| {
                b.breakdown
                    .vector_similarity
                    .total_cmp(&a.breakdown.vector_similarity)
            })
            .then_with(|| b.item.updated_at.cmp(&a.item.updated_at))
    });

    tracing::debug!(
        component = "RecallRunner",
        character_id = %input.character_id,
        reflection_count = reflections.len(),
        adjusted_count = scored
            .iter()
            .filter(|m| (m.breakdown.reflection_multiplier - 1.0).abs() > f32::EPSILON)
            .count(),
        "Applied self-reflection adjustment to recall scores"
    );
}

/// Bump access counters for the memories that were actually injected into the
/// prompt.
///
/// The bump is gated on `injected_ids` — the subset of recalled memories that
/// survived budget packing and were composed into the message packet — rather
/// than on "ranked high in search". Bumping every recalled memory would
/// reinforce memories that were recalled but then dropped, feeding the
/// self-reinforcing recall loop. Call this after prompt composition with
/// `PromptPacketMeta::injected_memory_ids`.
pub async fn bump_injected_memory_access(
    store: &dyn MemoryPort,
    cache: Option<&MemoryRecallCache>,
    injected_ids: &[i64],
) {
    let mut bumped = Vec::new();
    for &id in injected_ids {
        match store.bump_typed_memory_access(id).await {
            Ok(true) => bumped.push(id),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    component = "RecallRunner",
                    memory_id = id,
                    error = %error,
                    "Failed to bump typed memory access after prompt injection"
                );
            }
        }
    }
    if let Some(cache) = cache {
        // Single refresh after all bumps so the epoch gate covers every row
        // in one window instead of per id.
        cache.refresh_access(&bumped);
    }
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use crate::config::MindConfig;
    use ene_store::MemoryStore;
    use ene_store::{CommitmentStatus, NewCommitment};
    use ene_store::{MemoryConfidence, MemoryKind, MemorySalience, MemorySource, MemoryStatus};
    use ene_store::{MemoryScope, NewMemoryItem};

    #[tokio::test]
    async fn lorebook_rows_do_not_surface_in_recall_results() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "World tone".into(),
            content: "The world is always sunny.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("ccv3:lorebook:constant".into()),
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
            commitment_id: None,
        };
        let id = store.insert_typed_memory(&item).await.unwrap();
        store
            .upsert_memory_embedding(id, "mock", "content", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "How is the weather?",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        assert!(
            recalled
                .iter()
                .all(|m| m.item.content != "The world is always sunny."),
            "lorebook rows must not surface through recall; the injection path owns them"
        );
    }

    #[tokio::test]
    async fn cached_recall_matches_uncached_on_real_store() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let item = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: "User".into(),
            kind: MemoryKind::Preference,
            title: "coffee".into(),
            content: "The user likes coffee with oat milk.".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        let id = store.insert_typed_memory(&item).await.unwrap();
        store
            .upsert_memory_embedding(id, "mock", "content", &[1.0, 0.0, 0.0, 0.0])
            .await
            .unwrap();
        store
            .insert_commitment(&NewCommitment {
                character_id: "Ene".into(),
                user_id: "User".into(),
                title: "buy coffee".into(),
                description: "pick up beans tomorrow".into(),
                status: CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap();

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;

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
        let (_, second) = execute_hybrid_recall(&config, &input).await.unwrap();

        assert_eq!(first, second, "cached recall must equal uncached recall");
        let stats = cache.stats();
        assert_eq!(
            stats.misses, 3,
            "first run loads commitments/search/pending"
        );
        assert_eq!(stats.hits, 3, "repeat turn is served entirely from L1");
    }

    #[tokio::test]
    async fn lorebook_rows_do_not_consume_recall_slots() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedding = [1.0, 0.0, 0.0, 0.0];

        // A lorebook row whose embedding matches the query exactly. Without
        // the pre-MMR filter it would win the single seat and then be removed,
        // leaving nothing recalled at all.
        let lorebook = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "World tone".into(),
            content: "The world is always sunny.".into(),
            source: MemorySource::Ccv3,
            source_ref: Some("ccv3:lorebook:constant".into()),
            confidence: MemoryConfidence::new(1.0),
            salience: MemorySalience::new(1.0),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: true,
            created_at: None,
            commitment_id: None,
        };
        insert_with_embedding(&store, &lorebook, &embedding).await;

        let normal = NewMemoryItem {
            title: "user preference".into(),
            content: "The user likes tea.".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            ..lorebook.clone()
        };
        insert_with_embedding(&store, &normal, &[0.5, 0.0, 0.0, 0.0]).await;

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.recall_result_limit = 1;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "How is the weather?",
            recent_turns: &[],
            query_embedding: &embedding,
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        assert_eq!(
            recalled.len(),
            1,
            "the lone recall slot must go to a real memory, not a lorebook row"
        );
        assert_eq!(
            recalled[0].item.content, "The user likes tea.",
            "the lower-scoring normal memory must take the seat the lorebook row would have consumed"
        );
    }

    async fn insert_with_embedding(
        store: &MemoryStore,
        item: &NewMemoryItem,
        embedding: &[f32],
    ) -> i64 {
        let id = store.insert_typed_memory(item).await.unwrap();
        store
            .upsert_memory_embedding(id, "mock", "content", embedding)
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn reflection_boosts_recall_without_surfacing_as_result() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedding = [1.0, 0.0, 0.0, 0.0];

        // A normal memory that the reflection strategy names.
        let normal = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "warm greeting".into(),
            content: "greet the user warmly".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::new(0.6),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        insert_with_embedding(&store, &normal, &embedding).await;

        // A reflection memory naming "warm greeting" as a successful strategy.
        let reflection = NewMemoryItem {
            kind: MemoryKind::Reflection,
            title: "Successful strategies".into(),
            content: "Successful interaction strategies: warm greeting".into(),
            source: MemorySource::Inferred,
            ..normal.clone()
        };
        insert_with_embedding(&store, &reflection, &embedding).await;

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.reflection.enabled = true;
        config.memory.reflection.success_boost = 1.5;
        config.memory.reflection.failure_penalty = 0.5;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "warm greeting",
            recent_turns: &[],
            query_embedding: &embedding,
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");

        // The reflection memory is a scoring signal, never a recall result.
        assert!(
            recalled
                .iter()
                .all(|m| m.item.kind != MemoryKind::Reflection),
            "reflection memories must not surface as ordinary recall results"
        );

        // The matching normal memory carries the reflection boost.
        let boosted = recalled
            .iter()
            .find(|m| m.item.title == "warm greeting")
            .expect("normal memory recalled");
        assert!(
            (boosted.score_breakdown.reflection_multiplier - 1.5).abs() < f32::EPSILON,
            "reflection success_boost must be recorded in the breakdown"
        );
    }

    #[tokio::test]
    async fn reflection_disabled_leaves_scores_untouched() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let embedding = [1.0, 0.0, 0.0, 0.0];

        let normal = NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "Ene".into(),
            user_id: String::new(),
            kind: MemoryKind::Semantic,
            title: "warm greeting".into(),
            content: "greet the user warmly".into(),
            source: MemorySource::Conversation,
            source_ref: None,
            confidence: MemoryConfidence::new(0.8),
            salience: MemorySalience::new(0.6),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        };
        insert_with_embedding(&store, &normal, &embedding).await;

        let reflection = NewMemoryItem {
            kind: MemoryKind::Reflection,
            title: "Successful strategies".into(),
            content: "Successful interaction strategies: warm greeting".into(),
            source: MemorySource::Inferred,
            ..normal.clone()
        };
        insert_with_embedding(&store, &reflection, &embedding).await;

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.reflection.enabled = false;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "warm greeting",
            recent_turns: &[],
            query_embedding: &embedding,
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");

        assert!(
            recalled
                .iter()
                .all(|m| (m.score_breakdown.reflection_multiplier - 1.0).abs() < f32::EPSILON),
            "disabled reflection must not adjust recall scores"
        );
    }

    #[tokio::test]
    async fn pending_candidate_surfaces_when_topic_related_only() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();

        // A topic-related unconfirmed candidate: the arbiter deferred it with
        // AskConfirmationLater, so it sits in the pending queue.
        store
            .insert_pending_candidate(ene_core::PendingCandidate {
                id: 0,
                character_id: "Ene".into(),
                user_id: "User".into(),
                title: "favorite drink".into(),
                content: "the user's favorite drink is matcha".into(),
                kind: MemoryKind::Preference,
                confidence: 0.7,
                reason_detail: "ambiguous contradiction".into(),
                existing_memory_title: None,
                existing_memory_id: None,
                source_quote: "I like matcha".into(),
                status: ene_core::PendingCandidateStatus::Pending,
                created_at: now,
            })
            .await
            .unwrap();

        // A topic-unrelated pending candidate that must not surface (topic
        // gating): it shares no tokens at all with the query, so the lexical
        // overlap gate drops it even though `min_score` is 0.0 in this test.
        store
            .insert_pending_candidate(ene_core::PendingCandidate {
                id: 0,
                character_id: "Ene".into(),
                user_id: "User".into(),
                title: "fabrication".into(),
                content: "quantum entanglement fabrication procedures".into(),
                kind: MemoryKind::Semantic,
                confidence: 0.9,
                reason_detail: "ambiguous contradiction".into(),
                existing_memory_title: None,
                existing_memory_id: None,
                source_quote: "physics".into(),
                status: ene_core::PendingCandidateStatus::Pending,
                created_at: now,
            })
            .await
            .unwrap();

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "What does the user like to drink?",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");

        let pending: Vec<_> = recalled
            .iter()
            .filter(|m| {
                m.sources
                    .contains(&ene_core::MemoryCandidateSource::Pending)
            })
            .collect();
        assert_eq!(
            pending.len(),
            1,
            "only the topic-related pending candidate should surface, got {pending:?}"
        );
        assert!(pending[0].item.content.contains("matcha"));
        assert!(
            pending[0].item.id.is_none(),
            "pending candidates carry no typed id and must not be access-bumped"
        );
        assert!(
            !recalled.iter().any(|m| m.item.content.contains("quantum")),
            "topic-unrelated pending candidates must not surface"
        );
    }

    #[tokio::test]
    async fn pending_candidate_recall_respects_configured_limit() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();
        for i in 0..3i64 {
            store
                .insert_pending_candidate(ene_core::PendingCandidate {
                    id: 0,
                    character_id: "Ene".into(),
                    user_id: "User".into(),
                    title: format!("topic {i}"),
                    content: format!("topic related fact number {i}"),
                    kind: MemoryKind::Semantic,
                    confidence: 0.8,
                    reason_detail: "ambiguous contradiction".into(),
                    existing_memory_title: None,
                    existing_memory_id: None,
                    source_quote: "topic".into(),
                    status: ene_core::PendingCandidateStatus::Pending,
                    created_at: now,
                })
                .await
                .unwrap();
        }

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.recall_pending_candidate_limit = 1;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "User",
            user_input: "topic related",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");
        let pending = recalled
            .iter()
            .filter(|m| {
                m.sources
                    .contains(&ene_core::MemoryCandidateSource::Pending)
            })
            .count();
        assert_eq!(
            pending, 1,
            "configured limit must cap competing pending candidates"
        );
    }

    #[tokio::test]
    async fn pending_candidate_visibility_is_applied_before_the_cap() {
        let store = MemoryStore::open_in_memory(4).await.unwrap();
        let now = Utc::now();

        // Alice's topic-relevant candidate is older than Bob's two rows. With a
        // limit of 1, a newest-first truncate *before* the user-visibility
        // filter would let Bob's newer rows eat the cap and drop Alice's
        // candidate before it ever competes — the visibility
        // filter must run first so Alice's candidate still appears.
        store
            .insert_pending_candidate(ene_core::PendingCandidate {
                id: 0,
                character_id: "Ene".into(),
                user_id: "alice".into(),
                title: "topic alice".into(),
                content: "topic related fact for alice".into(),
                kind: MemoryKind::Semantic,
                confidence: 0.8,
                reason_detail: "ambiguous contradiction".into(),
                existing_memory_title: None,
                existing_memory_id: None,
                source_quote: "topic".into(),
                status: ene_core::PendingCandidateStatus::Pending,
                created_at: now - chrono::Duration::minutes(2),
            })
            .await
            .unwrap();
        for i in 0..2i64 {
            store
                .insert_pending_candidate(ene_core::PendingCandidate {
                    id: 0,
                    character_id: "Ene".into(),
                    user_id: "bob".into(),
                    title: format!("topic bob {i}"),
                    content: format!("topic related fact number {i}"),
                    kind: MemoryKind::Semantic,
                    confidence: 0.8,
                    reason_detail: "ambiguous contradiction".into(),
                    existing_memory_title: None,
                    existing_memory_id: None,
                    source_quote: "topic".into(),
                    status: ene_core::PendingCandidateStatus::Pending,
                    created_at: now,
                })
                .await
                .unwrap();
        }

        let mut config = MindConfig {
            language: "en".into(),
            ..MindConfig::default()
        };
        config.memory.recall_similarity_threshold = 0.0;
        config.memory.recall_min_score = 0.0;
        config.memory.recall_pending_candidate_limit = 1;

        let input = ExecuteRecallInput {
            store: &store,
            character_id: "Ene",
            user_id: "alice",
            user_input: "topic related",
            recent_turns: &[],
            query_embedding: &[1.0, 0.0, 0.0, 0.0],
            embedding_model: "mock",
            affect: None,
            cache: None,
            session_id: "sess",
        };

        let (_, recalled) = execute_hybrid_recall(&config, &input)
            .await
            .expect("recall");

        let pending: Vec<_> = recalled
            .iter()
            .filter(|m| {
                m.sources
                    .contains(&ene_core::MemoryCandidateSource::Pending)
            })
            .collect();
        assert_eq!(
            pending.len(),
            1,
            "alice's topic-relevant candidate must still appear after the cap, got {pending:?}"
        );
        assert!(
            pending[0].item.content.contains("alice"),
            "the surfaced candidate must be alice's, not bob's"
        );
        assert!(
            !recalled.iter().any(|m| m.item.user_id == "bob"),
            "bob's pending candidates must never leak into alice's recall"
        );
    }
}
