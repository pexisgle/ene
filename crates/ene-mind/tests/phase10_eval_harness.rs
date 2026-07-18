#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "integration tests use unwrap, fixed indices, and panic on invariant violations"
)]

use async_trait::async_trait;
use chrono::Utc;
use ene_ai::{
    EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk,
    embed_query,
};
use ene_config::CharacterCardV3;
use ene_mind::{CognitionEngine, HistoryEntry, MindConfig, TurnContext};
use ene_store::{
    AffectAnnotation, CommitmentStatus, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
    MemorySource, MemoryStatus, MemoryStore, NewCommitment, NewMemoryItem, PendingAffectProposal,
};
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;

struct EvalEmbedder;

#[async_trait]
impl EmbeddingProvider for EvalEmbedder {
    fn model_name(&self) -> &'static str {
        "eval-4d"
    }

    fn dimensions(&self) -> usize {
        4
    }

    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
        let mut out = Vec::with_capacity(items.len());
        for (text, _) in items {
            let t = text.to_lowercase();
            if t.contains("tea") || t.contains("matcha") {
                out.push(vec![1.0, 0.0, 0.0, 0.0]);
            } else if t.contains("movie") {
                out.push(vec![0.0, 1.0, 0.0, 0.0]);
            } else {
                out.push(vec![0.2, 0.2, 0.2, 0.2]);
            }
        }
        Ok(out)
    }
}

struct EvalLlm;

#[async_trait]
impl LlmProvider for EvalLlm {
    fn name(&self) -> &'static str {
        "eval-llm"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        Err(LlmProviderError::Provider(
            "not used in eval harness".into(),
        ))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        Ok("ok".into())
    }
}

struct PanicLlm;

#[async_trait]
impl LlmProvider for PanicLlm {
    fn name(&self) -> &'static str {
        "panic-llm"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        Err(LlmProviderError::Provider("not used".into()))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        panic!("before_turn must not call classifier chat_completion");
    }
}

fn eval_card() -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Always stay in character as Ene.".into();
    card.data.personality = "Supportive and curious.".into();
    card
}

async fn run_before_turn(
    engine: &CognitionEngine,
    store: &MemoryStore,
    config: &MindConfig,
    card: &CharacterCardV3,
    embedder: Arc<dyn EmbeddingProvider>,
    llm: Arc<dyn LlmProvider>,
    user_input: &str,
    history: &[HistoryEntry],
) -> ene_mind::PreTurnOutput {
    let query = embed_query(embedder.as_ref(), user_input).await.unwrap();
    let ctx = TurnContext {
        config,
        card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-eval",
        user_input,
        history,
        store: Some(store),
        query_embedding: Some(&query),
        embedder: Some(&embedder),
        llm_provider: Some(llm),
        post_history_block: None,
    };
    engine.before_turn(ctx).await.unwrap()
}

#[tokio::test]
async fn scenario_recall_relevant_memory_not_irrelevant() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();

    let tea = NewMemoryItem {
        scope: MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user".into(),
        kind: MemoryKind::Preference,
        title: "favorite tea".into(),
        content: "User likes matcha tea".into(),
        source: MemorySource::Conversation,
        source_ref: None,
        confidence: MemoryConfidence::new(0.9),
        salience: MemorySalience::new(0.8),
        affect: AffectAnnotation::default(),
        relationship_impact: 0.0,
        valid_from: None,
        valid_until: None,
        status: MemoryStatus::Active,
        supersedes_id: None,
        pinned: false,
        created_at: None,
        commitment_id: None,
    };
    let movie = NewMemoryItem {
        title: "favorite movie".into(),
        content: "User likes sci-fi movies".into(),
        ..tea.clone()
    };
    let tea_id = store.insert_typed_memory(&tea).await.unwrap();
    let movie_id = store.insert_typed_memory(&movie).await.unwrap();
    store
        .upsert_memory_embedding(tea_id, "eval-4d", "content", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();
    store
        .upsert_memory_embedding(movie_id, "eval-4d", "content", &[0.0, 1.0, 0.0, 0.0])
        .await
        .unwrap();

    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder,
        llm,
        "Do you remember my tea preference?",
        &[],
    )
    .await;

    let recalled_titles: Vec<String> = pre.recalled.iter().map(|r| r.item.title.clone()).collect();
    assert!(
        recalled_titles.iter().any(|t| t.contains("tea")),
        "expected tea memory in recall, got: {recalled_titles:?}"
    );
    assert!(
        recalled_titles.first().is_some_and(|t| t.contains("tea")),
        "relevant memory should rank first, got order: {recalled_titles:?}"
    );
}

#[tokio::test]
async fn scenario_identity_kernel_survives_long_history() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();

    let mut history = Vec::new();
    for i in 0..80 {
        history.push(HistoryEntry {
            role: if i % 2 == 0 {
                ene_ai::Role::User
            } else {
                ene_ai::Role::Assistant
            },
            content: format!("long conversation turn {i}"),
        });
    }
    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder.clone(),
        llm.clone(),
        "quick check",
        &history,
    )
    .await;
    let query = embed_query(embedder.as_ref(), "quick check").await.unwrap();
    let compose = engine
        .compose_prompt_packet(
            TurnContext {
                config: &config,
                card: &card,
                character_id: "ene",
                user_name: "user",
                session_id: "sess-eval",
                user_input: "quick check",
                history: &history,
                store: Some(&store),
                query_embedding: Some(&query),
                embedder: Some(&embedder),
                llm_provider: Some(llm),
                post_history_block: None,
            },
            &pre,
            ene_mind::ComposePrefetch::default(),
        )
        .await
        .unwrap();

    let LlmMessage::System { content } = &compose.messages[0] else {
        panic!("first message must be system");
    };
    assert!(
        content.contains("Ene") && content.contains("Always stay in character"),
        "identity kernel missing from prompt packet: {content}"
    );
}

#[tokio::test]
async fn scenario_explicit_forget_respected_by_recall() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();

    let item = NewMemoryItem {
        scope: MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user".into(),
        kind: MemoryKind::Preference,
        title: "favorite tea".into(),
        content: "User likes matcha tea".into(),
        source: MemorySource::Conversation,
        source_ref: None,
        confidence: MemoryConfidence::new(0.9),
        salience: MemorySalience::new(0.8),
        affect: AffectAnnotation::default(),
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
        .upsert_memory_embedding(id, "eval-4d", "content", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();
    store
        .transition_typed_memory_status(id, MemoryStatus::UserDeleted)
        .await
        .unwrap();

    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder,
        llm,
        "what tea do I like?",
        &[],
    )
    .await;
    let recalled_ids: Vec<i64> = pre.recalled.iter().filter_map(|r| r.item.id).collect();
    assert!(
        !recalled_ids.contains(&id),
        "user_deleted memory should not be recalled, got ids: {recalled_ids:?}"
    );
}

#[tokio::test]
async fn scenario_active_commitments_are_prompt_candidates() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();

    store
        .insert_commitment(&NewCommitment {
            character_id: "ene".into(),
            user_id: "user".into(),
            title: "Follow up on tea recommendation".into(),
            description: "Bring this up next session".into(),
            status: CommitmentStatus::Active,
            due_at: None,
            due_label: None,
        })
        .await
        .unwrap();

    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder,
        llm,
        "hello again",
        &[],
    )
    .await;
    assert!(
        !pre.commitments.is_empty(),
        "expected active commitment candidates for prompt section"
    );
}

#[tokio::test]
async fn scenario_affect_update_is_deterministic() {
    let store_a = MemoryStore::open_in_memory(4).await.unwrap();
    let store_b = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();
    let history = [HistoryEntry {
        role: ene_ai::Role::User,
        content: "I had a wonderful day".into(),
    }];

    let pre_a = run_before_turn(
        &engine,
        &store_a,
        &config,
        &card,
        embedder.clone(),
        llm.clone(),
        "I am so happy today",
        &history,
    )
    .await;
    let pre_b = run_before_turn(
        &engine,
        &store_b,
        &config,
        &card,
        embedder,
        llm,
        "I am so happy today",
        &history,
    )
    .await;

    assert!(
        (pre_a.affect.valence - pre_b.affect.valence).abs() < f32::EPSILON
            && (pre_a.affect.arousal - pre_b.affect.arousal).abs() < f32::EPSILON
            && pre_a.affect.mood_label == pre_b.affect.mood_label,
        "affect update should be deterministic: left={:?}, right={:?}",
        pre_a.affect,
        pre_b.affect
    );
}

#[tokio::test]
async fn scenario_before_turn_does_not_block_on_classifier() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(PanicLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();

    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder,
        llm,
        "thanks for helping me",
        &[],
    )
    .await;
    assert!(
        !pre.affect.mood_label.is_empty(),
        "before_turn should complete without classifier provider call"
    );
}

#[tokio::test]
async fn scenario_pending_classifier_proposal_applies_once() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();
    store
        .upsert_pending_affect_proposal(&PendingAffectProposal {
            character_id: "ene".into(),
            user_id: "user".into(),
            source_turn_id: 1,
            user_emotion: "happy".into(),
            user_intent: "praise".into(),
            valence: 0.3,
            arousal: 0.0,
            irritation: 0.0,
            affinity: 0.2,
            recommended_expression: "happy".into(),
            confidence: 0.9,
            reason: "test".into(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let history = vec![
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "previous turn".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: "previous reply".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "hello again".into(),
        },
    ];
    let pre = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder.clone(),
        llm.clone(),
        "hello again",
        &history,
    )
    .await;
    assert_eq!(pre.classifier_expression_hint.as_deref(), Some("happy"));
    assert!(
        pre.affect.valence > 0.0,
        "pending classifier estimate should affect next turn"
    );

    let pending = store
        .get_pending_affect_proposal("ene", "user")
        .await
        .unwrap();
    assert!(pending.is_none(), "proposal must be consumed once");

    let pre_second = run_before_turn(
        &engine,
        &store,
        &config,
        &card,
        embedder,
        llm,
        "third turn",
        &[
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "previous turn".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::Assistant,
                content: "previous reply".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "hello again".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::Assistant,
                content: "second reply".into(),
            },
            HistoryEntry {
                role: ene_ai::Role::User,
                content: "third turn".into(),
            },
        ],
    )
    .await;
    assert!(
        pre_second.classifier_expression_hint.is_none(),
        "consumed proposal should not be applied again"
    );
}

#[tokio::test]
async fn scenario_stale_pending_classifier_proposal_is_dropped() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();
    store
        .upsert_pending_affect_proposal(&PendingAffectProposal {
            character_id: "ene".into(),
            user_id: "user".into(),
            source_turn_id: 1,
            user_emotion: "happy".into(),
            user_intent: "praise".into(),
            valence: 0.3,
            arousal: 0.0,
            irritation: 0.0,
            affinity: 0.2,
            recommended_expression: "happy".into(),
            confidence: 0.9,
            reason: "stale".into(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let history = vec![
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "turn 1".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: "reply 1".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "turn 2".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: "reply 2".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "turn 3".into(),
        },
    ];
    let pre = run_before_turn(
        &engine, &store, &config, &card, embedder, llm, "turn 3", &history,
    )
    .await;

    assert!(
        pre.classifier_expression_hint.is_none(),
        "stale proposal from turn 1 should not apply on turn 3"
    );
    assert!(
        store
            .get_pending_affect_proposal("ene", "user")
            .await
            .unwrap()
            .is_none(),
        "stale proposal should be consumed and dropped"
    );
}

#[tokio::test]
async fn scenario_future_pending_classifier_proposal_is_dropped() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(EvalEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(EvalLlm);
    let engine = CognitionEngine::new();
    let config = MindConfig::default();
    let card = eval_card();
    store
        .upsert_pending_affect_proposal(&PendingAffectProposal {
            character_id: "ene".into(),
            user_id: "user".into(),
            source_turn_id: 2,
            user_emotion: "happy".into(),
            user_intent: "praise".into(),
            valence: 0.3,
            arousal: 0.0,
            irritation: 0.0,
            affinity: 0.2,
            recommended_expression: "happy".into(),
            confidence: 0.9,
            reason: "future".into(),
            created_at: Utc::now(),
        })
        .await
        .unwrap();

    let history = vec![
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "turn 1".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::Assistant,
            content: "reply 1".into(),
        },
        HistoryEntry {
            role: ene_ai::Role::User,
            content: "turn 2".into(),
        },
    ];
    let pre = run_before_turn(
        &engine, &store, &config, &card, embedder, llm, "turn 2", &history,
    )
    .await;

    assert!(
        pre.classifier_expression_hint.is_none(),
        "future proposal tagged for turn 2 should not apply on turn 2 pre-turn"
    );
}
