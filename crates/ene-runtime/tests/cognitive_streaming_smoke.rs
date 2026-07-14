//! Integration smoke tests for cognitive runtime streaming (#100).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use ene_ai::{
    EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk,
    embed_query,
};
use ene_config::{CharacterCardV3, PromptLibrary, expand_cbs_macros};
use ene_mind::{CognitionEngine, HistoryEntry, MindConfig, TurnContext};
use ene_runtime::message_builder::{build_cognitive_output_contract, build_expression_phi};
use ene_store::{
    AffectAnnotation, AffectState, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope,
    MemorySource, MemoryStatus, MemoryStore, NewMemoryItem,
};
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;

struct MockEmbedder;

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    fn model_name(&self) -> &str {
        "mock-4d"
    }

    fn dimensions(&self) -> usize {
        4
    }

    async fn embed_batch(
        &self,
        items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
        Ok(items.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
    }
}

struct MockLlm;

#[async_trait]
impl LlmProvider for MockLlm {
    fn name(&self) -> &str {
        "mock"
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
        Ok("ok".into())
    }
}

#[tokio::test]
async fn cognitive_lifecycle_compose_prompt_includes_identity_kernel_and_recall() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let item = NewMemoryItem {
        scope: MemoryScope::User,
        character_id: "ene".into(),
        user_id: "user".into(),
        kind: MemoryKind::Preference,
        title: "favorite drink".into(),
        content: "matcha latte".into(),
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
        .upsert_memory_embedding(id, "mock-4d", "content", &[1.0, 0.0, 0.0, 0.0])
        .await
        .unwrap();

    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();
    card.data.personality = "Cheerful.".into();

    let mind = MindConfig::default();
    let engine = CognitionEngine::new();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlm);
    let query_emb = embed_query(embedder.as_ref(), "matcha").await.unwrap();

    let turn_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-1",
        user_input: "Do you remember my favorite drink?",
        history: &[],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm.clone()),
        post_history_block: None,
    };

    let pre = engine.before_turn(turn_ctx).await.expect("before_turn");
    assert!(
        !pre.recalled.is_empty() || pre.recall_plan.search.primary_query_text.contains("matcha")
    );

    let compose_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-1",
        user_input: "Do you remember my favorite drink?",
        history: &[HistoryEntry {
            role: ene_ai::Role::User,
            content: "hello".into(),
        }],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm),
        post_history_block: None,
    };

    let composed = engine
        .compose_prompt_packet(compose_ctx, &pre)
        .await
        .expect("compose");

    assert!(composed.meta.identity_kernel_included);
    let LlmMessage::System { content } = &composed.messages[0] else {
        panic!("expected system message");
    };
    assert!(content.contains("Ene"));
    assert!(content.contains("Be helpful."));
    if composed.meta.recalled_memory_count > 0 {
        assert!(content.contains("matcha"));
    }

    let affect_before = store.get_affect_state("ene").await.unwrap();
    let post = ene_mind::PostTurnInput {
        turn: ene_mind::memory_writer::candidate::TurnInput {
            user_message: "Remember that I like matcha",
            assistant_message: Some("Got it!"),
            tool_results: &[],
        },
        affect: affect_before.clone(),
        character_id: "ene",
        user_id: "User",
    };
    engine
        .after_turn(
            &store,
            &mind,
            post,
            ene_mind::memory_writer::MemoryWriteProviders::default(),
        )
        .await
        .expect("after_turn");
    let loaded = store.get_affect_state("ene").await.unwrap();
    assert_eq!(loaded.character_id, "ene");
    assert_eq!(loaded.mood_label, affect_before.mood_label);
}

#[tokio::test]
async fn cognitive_compose_includes_post_history_phi_block() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.post_history_instructions = "Stay in character at all times.".into();

    let mind = MindConfig::default();
    let engine = CognitionEngine::new();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlm);
    let query_emb = embed_query(embedder.as_ref(), "hello").await.unwrap();

    let prompts = PromptLibrary::load("en");
    let phi =
        build_expression_phi(&card, &prompts).map(|block| expand_cbs_macros(&block, "Ene", "user"));

    let pre_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-phi",
        user_input: "hello",
        history: &[],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm.clone()),
        post_history_block: phi.as_deref(),
    };
    let pre = engine.before_turn(pre_ctx).await.expect("before_turn");

    let compose_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-phi",
        user_input: "hello",
        history: &[],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm),
        post_history_block: phi.as_deref(),
    };
    let composed = engine
        .compose_prompt_packet(compose_ctx, &pre)
        .await
        .expect("compose");

    assert!(composed.meta.post_history_included);
    let phi_message = composed
        .messages
        .iter()
        .find(|msg| matches!(msg, LlmMessage::System { content } if content.contains("Stay in character")));
    assert!(
        phi_message.is_some(),
        "post-history PHI should appear as a system message"
    );
}

#[tokio::test]
async fn cognitive_compose_includes_active_scene_summary() {
    use ene_store::NewMemorySpan;

    let store = MemoryStore::open_in_memory(4).await.unwrap();
    store
        .insert_memory_span(&NewMemorySpan {
            session_id: "sess-scene".into(),
            turn_start: 0,
            turn_end: 4,
            raw_excerpt: Some("user: hi\nassistant: hello".into()),
            compressed_summary: Some("They greeted each other warmly.".into()),
            compression_level: 0,
        })
        .await
        .unwrap();

    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mind = MindConfig::default();
    let engine = CognitionEngine::new();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlm);
    let query_emb = embed_query(embedder.as_ref(), "hello").await.unwrap();

    let pre_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-scene",
        user_input: "hello again",
        history: &[],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm.clone()),
        post_history_block: None,
    };
    let pre = engine.before_turn(pre_ctx).await.expect("before_turn");

    let compose_ctx = TurnContext {
        config: &mind,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-scene",
        user_input: "hello again",
        history: &[],
        store: Some(&store),
        query_embedding: Some(&query_emb),
        embedder: Some(&embedder),
        llm_provider: Some(llm),
        post_history_block: None,
    };
    let composed = engine
        .compose_prompt_packet(compose_ctx, &pre)
        .await
        .expect("compose");

    assert!(composed.meta.scene_summary_included);
    let LlmMessage::System { content } = &composed.messages[0] else {
        panic!("expected system message");
    };
    assert!(content.contains("greeted each other"));
}

#[test]
fn cognitive_output_contract_uses_natural_dialogue_when_emotion_enabled() {
    let card = CharacterCardV3::default();
    let prompts = PromptLibrary::load("en");
    let contract =
        build_cognitive_output_contract(&card, &prompts, true, "Alice").expect("contract");
    assert!(contract.contains("natural dialogue") || contract.contains("Output Contract"));
    assert!(contract.contains("Do NOT emit") || contract.contains("トークン"));
}

#[test]
fn cognitive_output_contract_uses_phi_when_emotion_disabled() {
    let card = CharacterCardV3::default();
    let prompts = PromptLibrary::load("en");
    let contract = build_cognitive_output_contract(&card, &prompts, false, "Alice");
    // Default card includes built-in expressions via resolve_expressions.
    if let Some(phi) = contract {
        assert!(
            phi.contains("<|perf:")
                || phi.contains("<|emo:")
                || phi.contains("Emotion")
                || phi.contains("emotion")
        );
    }
}

#[test]
fn expression_resolves_from_classifier_hint_without_stream_token() {
    let mind = MindConfig::default();
    let engine = CognitionEngine::new();
    let card = CharacterCardV3::default();
    let mut affect = AffectState::neutral("ene");
    affect.valence = 0.1;
    affect.arousal = 0.0;

    let (decision, _) = engine.resolve_expression_turn(
        &mind,
        &card,
        &affect,
        "That sounds great!",
        Some("happy"),
        "",
        None,
    );
    assert_eq!(decision.expression, "happy");
}

#[tokio::test]
async fn persist_affect_snapshot_writes_before_finalize() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mut affect = AffectState::neutral("ene");
    affect.valence = 0.42;

    CognitionEngine::persist_affect_snapshot(&store, &affect)
        .await
        .expect("persist snapshot");

    let loaded = store.get_affect_state("ene").await.unwrap();
    assert!((loaded.valence - 0.42).abs() < f32::EPSILON);
}

#[tokio::test]
async fn post_turn_tool_results_persist_procedure_memory() {
    let store = MemoryStore::open_in_memory(4).await.unwrap();
    let mind = MindConfig::default();
    let engine = CognitionEngine::new();
    let affect = AffectState::neutral("ene");
    let tool_results = vec![ene_mind::memory_writer::ToolResultSummary {
        tool_name: "fs".into(),
        success: true,
        summary: "wrote notes.txt".into(),
    }];
    let post = ene_mind::PostTurnInput {
        turn: ene_mind::memory_writer::candidate::TurnInput {
            user_message: "save this",
            assistant_message: Some("saved"),
            tool_results: &tool_results,
        },
        affect,
        character_id: "ene",
        user_id: "User",
    };
    engine
        .after_turn(
            &store,
            &mind,
            post,
            ene_mind::memory_writer::MemoryWriteProviders::default(),
        )
        .await
        .expect("after_turn");

    let grounded = store
        .list_typed_memories_by_source_prefix("ene", "tool:", 16)
        .await
        .expect("list by source prefix");
    assert!(
        grounded.iter().any(|m| m.kind == MemoryKind::Procedure),
        "expected Procedure memory from tool grounding, got {grounded:?}"
    );
}
