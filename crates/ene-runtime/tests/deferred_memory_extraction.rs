//! Deferred post-turn memory extraction must not block Terminal (#100).

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
use ene_mind::MindConfig;
use ene_runtime::streaming::{StreamContext, run_stream_cognitive};
use ene_runtime::{EneEvent, TerminalReason, TurnOrigin};
use ene_store::MemoryStore;
use ene_tool_host::{ToolHostError, ToolRegistry};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
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

struct SlowExtractionLlm {
    extraction_delay: Duration,
}

#[async_trait]
impl LlmProvider for SlowExtractionLlm {
    fn name(&self) -> &'static str {
        "slow-extraction-mock"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let chunks = vec![Ok(LlmResponseChunk {
            text_delta: Some("Hello back.".into()),
            tool_calls_delta: None,
        })];
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        tokio::time::sleep(self.extraction_delay).await;
        Ok(r#"{"candidates": []}"#.into())
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
async fn terminal_does_not_wait_for_deferred_memory_extraction() {
    let store = Arc::new(MemoryStore::open_in_memory(4).await.unwrap());
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);
    session.memory.memory_store = Some(store);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.emotion.enabled = false;
    config.set_section(&mind).expect("mind config");
    let mut mem_config = ene_store::StoreConfig::default();
    mem_config.enabled = true;
    config.set_section(&mem_config).expect("memory config");

    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(64);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(16);
    let (memory_writer_tx, mut memory_writer_rx) = tokio::sync::mpsc::unbounded_channel();
    let terminal_emitted = Arc::new(AtomicBool::new(false));
    let turn = ene_runtime::TurnId::new();

    let ctx = StreamContext {
        config,
        session,
        user_input: "Hello".into(),
        embedder: Some(Arc::new(MockEmbedder)),
        registry: Arc::new(EmptyRegistry) as Arc<dyn ToolRegistry>,
        tool_rag: None,
        provider: Arc::new(SlowExtractionLlm {
            extraction_delay: Duration::from_secs(2),
        }),
        event_tx,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        terminal_emitted,
        turn: turn.clone(),
        origin: TurnOrigin::User,
        allow_tools: false,
        runtime_directive: None,
        generation_timeout: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx,
    };

    let started = Instant::now();
    let outcome = run_stream_cognitive(ctx).await;
    let elapsed = started.elapsed();

    assert_eq!(outcome.terminal, TerminalReason::Done);
    assert!(
        elapsed < Duration::from_secs(1),
        "stream should finish before slow extraction ({elapsed:?})"
    );
    assert!(
        outcome
            .session
            .history()
            .iter()
            .any(|entry| entry.content.contains("Hello back.")),
        "assistant reply should be committed to session history before Terminal"
    );

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

    let writer_handle = memory_writer_rx
        .recv()
        .await
        .expect("deferred memory writer handle");
    let writer_started = Instant::now();
    writer_handle.await.expect("writer task join");
    assert!(
        writer_started.elapsed() >= Duration::from_millis(1500),
        "deferred extraction should still run to completion in the background"
    );
}
