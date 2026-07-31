//! Engine-level integration test for the recall access bump (#345).
//!
//! The bump is gated on `PromptPacketMeta::injected_memory_ids` — the recalled
//! memories actually composed into the packed prompt. These tests run the full
//! path through `CognitionEngine::compose_prompt_packet` against a real
//! in-memory store and assert that injected memories get their access counters
//! bumped while recalled-but-trimmed or recalled-but-dropped memories do not.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::default_trait_access,
    reason = "integration tests use unwrap/expect and Default for concise fixtures"
)]

use ene_core::{AffectState, MemoryItem, MemoryScoreBreakdown, MemoryStatus};
use ene_mind::recall::{
    RecallBudgetHints, RecallPlan, RecallReason, RecallScopeFilter, RecallSearchHints,
    RecalledMemory,
};
use ene_mind::{
    CognitionEngine, ComposePrefetch, ComposedPrompt, ContextConfig, MindConfig, PreTurnOutput,
    TurnContext,
};
use ene_store::{
    MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource, MemoryStore,
    NewMemoryItem,
};

/// Wrap a stored `MemoryItem` in a recalled-memory DTO for prompt injection.
fn build_recalled(item: MemoryItem, reason: RecallReason) -> RecalledMemory {
    let confidence = item.confidence.get();
    RecalledMemory {
        item,
        reason,
        score_breakdown: MemoryScoreBreakdown {
            vector_similarity: 0.8,
            lexical_score: 0.0,
            recency_score: 0.0,
            salience: 0.0,
            confidence,
            emotional_match: 0.0,
            relationship: 0.0,
            access_boost: 0.0,
            relevance: confidence,
            quality_factor: 1.0,
            contradiction_penalty: 0.0,
            stale_penalty: 0.0,
            commitment_boost: 0.0,
            total: confidence,
        },
        sources: vec![],
    }
}

/// Insert a typed memory and return its assigned ID.
async fn insert_memory(
    store: &MemoryStore,
    kind: MemoryKind,
    source: MemorySource,
    content: &str,
    confidence: f32,
) -> i64 {
    store
        .insert_typed_memory(&NewMemoryItem {
            scope: MemoryScope::Character,
            character_id: "ene".into(),
            user_id: "user".into(),
            kind,
            title: "t".into(),
            content: content.into(),
            source,
            source_ref: None,
            confidence: MemoryConfidence::new(confidence),
            salience: MemorySalience::new(0.8),
            affect: Default::default(),
            relationship_impact: 0.0,
            valid_from: None,
            valid_until: None,
            status: MemoryStatus::Active,
            supersedes_id: None,
            pinned: false,
            created_at: None,
            commitment_id: None,
        })
        .await
        .unwrap()
}

/// Build a config whose sub-budgets satisfy the `max_prompt_tokens` validation
/// (`scene + memory + semantic + style <= total`).
fn test_config(max_prompt_tokens: usize, memory_budget_tokens: usize) -> MindConfig {
    MindConfig {
        context: ContextConfig {
            max_prompt_tokens,
            scene_summary_tokens: 10,
            memory_budget_tokens,
            semantic_budget_tokens: 10,
            style_example_budget_tokens: 10,
            ..ContextConfig::default()
        },
        ..MindConfig::default()
    }
}

/// Compose a prompt packet for a turn with the given recalled memories.
async fn compose(
    engine: &CognitionEngine,
    config: &MindConfig,
    store: Option<&dyn ene_core::MemoryPort>,
    recalled: Vec<RecalledMemory>,
    memory_budget_tokens: usize,
    semantic_budget_tokens: usize,
) -> ComposedPrompt {
    let card = ene_config::CharacterCardV3::default();
    let pre = PreTurnOutput {
        recall_plan: RecallPlan {
            current_topic: "test".into(),
            semantic_queries: Vec::new(),
            episodic_queries: Vec::new(),
            required_kinds: Vec::new(),
            scope: RecallScopeFilter {
                character_id: "ene".into(),
                user_id: Some("user".into()),
            },
            budget: RecallBudgetHints {
                memory_budget_tokens,
                semantic_budget_tokens,
                result_limit: 8,
            },
            search: RecallSearchHints {
                primary_query_text: "test".into(),
                similarity_threshold: 0.0,
                min_score: 0.0,
                decay_half_life_days: 30.0,
                query_affect: None,
            },
        },
        affect: AffectState::neutral("ene"),
        recalled,
        commitments: Vec::new(),
        classifier_expression_hint: None,
    };
    let prefetch = ComposePrefetch {
        style_examples: Some(vec![]),
        scene_summary: Some(None),
        interruption_note: None,
    };
    let ctx = TurnContext {
        config,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess",
        user_input: "hi",
        history: &[],
        store,
        query_embedding: None,
        embedder: None,
        llm_provider: None,
        post_history_block: None,
    };
    engine
        .compose_prompt_packet(ctx, pre, prefetch)
        .await
        .expect("compose_prompt_packet")
}

#[tokio::test]
async fn injected_memories_are_bumped_and_truncated_ones_are_not() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let engine = CognitionEngine::new();

    // Two episodic memories with identical bullet sizes: a 6-token section
    // budget (24 chars) fits one 16-char bullet but not two, so the
    // low-confidence memory is trimmed at build time and the high-confidence
    // one survives.
    let survivor_id = insert_memory(
        &store,
        MemoryKind::Episodic,
        MemorySource::Conversation,
        "m1 m1 m1 m1 m1",
        0.9,
    )
    .await;
    let trimmed_id = insert_memory(
        &store,
        MemoryKind::Episodic,
        MemorySource::Conversation,
        "m2 m2 m2 m2 m2",
        0.5,
    )
    .await;

    let survivor = store.get_typed_memory(survivor_id).await.unwrap().unwrap();
    let trimmed = store.get_typed_memory(trimmed_id).await.unwrap().unwrap();
    let recalled = vec![
        build_recalled(trimmed, RecallReason::SimilarTopic),
        build_recalled(survivor, RecallReason::SimilarTopic),
    ];

    let config = test_config(100_000, 6);
    let composed = compose(&engine, &config, Some(&store), recalled, 6, 100).await;

    assert_eq!(
        composed.meta.injected_memory_ids,
        vec![survivor_id],
        "only the memory that survived the section budget may be injected"
    );
    assert!(
        composed.meta.dropped_sections.is_empty(),
        "a generous total budget should not drop any section"
    );

    let survivor = store.get_typed_memory(survivor_id).await.unwrap().unwrap();
    let trimmed = store.get_typed_memory(trimmed_id).await.unwrap().unwrap();
    assert_eq!(
        survivor.access_count, 1,
        "the injected memory must be bumped"
    );
    assert!(survivor.last_accessed_at.is_some());
    assert_eq!(
        trimmed.access_count, 0,
        "the section-budget-trimmed memory must not be bumped"
    );
    assert!(trimmed.last_accessed_at.is_none());
}

#[tokio::test]
async fn dropped_section_memories_are_not_bumped() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let engine = CognitionEngine::new();

    // A small semantic memory (survives the drop pass) and a large episodic
    // memory whose section is dropped under the tight total budget.
    let semantic_id = insert_memory(
        &store,
        MemoryKind::Semantic,
        MemorySource::Ccv3,
        "prefers tea",
        0.9,
    )
    .await;
    let episodic_id = insert_memory(
        &store,
        MemoryKind::Episodic,
        MemorySource::Conversation,
        &"episodic filler content ".repeat(15),
        0.8,
    )
    .await;

    // Measure the no-recall baseline so the tight total budget is relative to
    // the actual kernel/affect/platform overhead rather than hardcoded token
    // estimates that would depend on the card defaults.
    let baseline = {
        let config = test_config(100_000, 100);
        let composed = compose(&engine, &config, Some(&store), vec![], 100, 100).await;
        composed.meta.packed_tokens
    };

    let semantic = store.get_typed_memory(semantic_id).await.unwrap().unwrap();
    let episodic = store.get_typed_memory(episodic_id).await.unwrap().unwrap();
    let recalled = vec![
        build_recalled(semantic, RecallReason::CharacterLore),
        build_recalled(episodic, RecallReason::SimilarTopic),
    ];

    // Baseline + 20 tokens fits the surviving sections; the ~90-token episodic
    // section pushes the packet over, so the drop loop clears it.
    let config = test_config(baseline + 20, 6);
    let composed = compose(&engine, &config, Some(&store), recalled, 100, 100).await;

    assert!(
        composed
            .meta
            .dropped_sections
            .contains(&ene_mind::prompt_packet::PromptSectionKind::EpisodicMemories),
        "episodic section should be dropped under the tight total budget, dropped={:?}",
        composed.meta.dropped_sections
    );
    assert_eq!(
        composed.meta.injected_memory_ids,
        vec![semantic_id],
        "only the surviving semantic memory may be injected"
    );

    let semantic = store.get_typed_memory(semantic_id).await.unwrap().unwrap();
    let episodic = store.get_typed_memory(episodic_id).await.unwrap().unwrap();
    assert_eq!(
        semantic.access_count, 1,
        "the injected memory must be bumped"
    );
    assert_eq!(
        episodic.access_count, 0,
        "a recalled-but-dropped memory must not be bumped"
    );
    assert!(episodic.last_accessed_at.is_none());
}
