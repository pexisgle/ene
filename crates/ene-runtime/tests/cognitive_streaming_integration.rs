//! Integration smoke test for cognitive streaming via `run_stream` (#100).

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::field_reassign_with_default,
    reason = "integration tests use unwrap/expect and default-then-assign test fixtures"
)]

use async_trait::async_trait;
use ene_ai::{
    EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk,
};
use ene_config::{CharacterCardV3, EneConfig};
use ene_runtime::streaming::{StreamContext, run_stream_cognitive};
use ene_runtime::{EneEvent, TerminalReason};
use ene_store::MemoryStore;
use ene_tool_host::{ToolHostError, ToolRegistry};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

struct MockEmbedder;

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    fn model_name(&self) -> &'static str {
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

struct MockLlm {
    response: String,
}

#[async_trait]
impl LlmProvider for MockLlm {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        assert!(
            messages
                .iter()
                .any(|m| matches!(m, LlmMessage::System { content } if content.contains("Ene"))),
            "identity kernel should be in system prompt"
        );

        let response = self.response.clone();
        let chunks = vec![Ok(LlmResponseChunk {
            text_delta: Some(response),
            tool_calls_delta: None,
        })];
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        Ok(self.response.clone())
    }
}

struct EmptyRegistry;

#[async_trait]
impl ToolRegistry for EmptyRegistry {
    fn list_tools(&self) -> Vec<ene_tool_proto::ToolSpec> {
        vec![]
    }

    async fn call_tool(&self, _name: &str, _arguments: &str) -> Result<String, ToolHostError> {
        Err(ToolHostError::ExecutionFailed {
            message: "not used".into(),
        })
    }
}

#[tokio::test]
async fn run_stream_cognitive_path_completes_with_logs() {
    let store = Arc::new(MemoryStore::open_in_memory(4).await.unwrap());
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);
    session.memory.memory_store = Some(store.clone());

    let mut config = EneConfig::default();
    let mut mem_config = ene_store::StoreConfig::default();
    mem_config.enabled = true;
    config.set_section(&mem_config).expect("memory config");
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(64);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(16);
    let terminal_emitted = Arc::new(AtomicBool::new(false));
    let turn = ene_runtime::TurnId::new();

    let ctx = StreamContext {
        config,
        session,
        user_input: "Hello Ene".into(),
        embedder: Some(Arc::new(MockEmbedder)),
        registry: Arc::new(EmptyRegistry) as Arc<dyn ToolRegistry>,
        tool_rag: None,
        provider: Arc::new(MockLlm {
            response: "Hi there!".into(),
        }),
        event_tx,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        terminal_emitted,
        turn: turn.clone(),
        origin: ene_runtime::TurnOrigin::User,
        allow_tools: true,
        runtime_directive: None,
        proactive_screen_image: None,
        generation_timeout: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
    };

    let _session = run_stream_cognitive(ctx).await;

    let mut saw_done = false;
    while let Ok(event) = event_rx.try_recv() {
        if let EneEvent::Terminal {
            turn: ref t,
            origin: _,
            reason: TerminalReason::Done,
        } = event
        {
            assert_eq!(t, &turn);
            saw_done = true;
        }
    }
    assert!(saw_done, "expected terminal Done event");

    let sessions = store.list_session_ids_for_card("Ene").await.unwrap();
    assert!(!sessions.is_empty(), "conversation logs should be saved");
    let logs = store.get_logs_by_session(&sessions[0]).await.unwrap();
    assert!(logs.len() >= 2, "user and assistant logs should be saved");
}
