//! Integration smoke tests for cognitive runtime streaming (#100).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use ene_cognition::{CognitionConfig, CognitionEngine, HistoryEntry, TurnContext};
use ene_config::{CharacterCardV3, PromptLibrary, expand_cbs_macros};
use ene_core::message_builder::build_expression_phi;
use ene_memory::{
    AffectAnnotation, MemoryConfidence, MemoryKind, MemorySalience, MemoryScope, MemorySource,
    MemoryStatus, MemoryStore, NewMemoryItem,
};
use ene_provider::{
    EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk,
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

    async fn embed(
        &self,
        _text: &str,
        _kind: EmbeddingKind,
    ) -> Result<Vec<f32>, ene_provider::EmbeddingError> {
        Ok(vec![1.0, 0.0, 0.0, 0.0])
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

    let cognition = CognitionConfig::default();
    let engine = CognitionEngine::new();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlm);
    let query_emb = embedder.embed_query("matcha").await.unwrap();

    let turn_ctx = TurnContext {
        config: &cognition,
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
        config: &cognition,
        card: &card,
        character_id: "ene",
        user_name: "user",
        session_id: "sess-1",
        user_input: "Do you remember my favorite drink?",
        history: &[HistoryEntry {
            role: "user".into(),
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
    let post = ene_cognition::PostTurnInput {
        turn: ene_cognition::memory_writer::candidate::TurnInput {
            user_message: "Remember that I like matcha",
            assistant_message: Some("Got it!"),
            tool_results: &[],
        },
        affect: affect_before.clone(),
        character_id: "ene",
        user_id: "User",
    };
    engine
        .after_turn(&store, &cognition, post)
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

    let cognition = CognitionConfig::default();
    let engine = CognitionEngine::new();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder);
    let llm: Arc<dyn LlmProvider> = Arc::new(MockLlm);
    let query_emb = embedder.embed_query("hello").await.unwrap();

    let prompts = PromptLibrary::load("en");
    let phi =
        build_expression_phi(&card, &prompts).map(|block| expand_cbs_macros(&block, "Ene", "user"));

    let pre_ctx = TurnContext {
        config: &cognition,
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
        config: &cognition,
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
