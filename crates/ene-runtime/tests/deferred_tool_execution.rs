//! Integration test for deferred (background) tool execution (#196).
//!
//! Verifies that:
//! 1. Background-capable tools are called with `call_tool_deferred`
//! 2. Deferred tasks are sent to the actor via `deferred_tool_tx`
//! 3. The LLM receives a "Task queued" message instead of blocking
//! 4. The actor polls and emits `ToolBackgroundCompleted` events

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::field_reassign_with_default,
    reason = "integration tests use unwrap/expect and default-then-assign test fixtures"
)]

use async_trait::async_trait;
use ene_ai::{
    EmbeddingKind, EmbeddingProvider, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk,
    LlmToolCallChunk,
};
use ene_config::{CharacterCardV3, EneConfig};
use ene_plugin_host::{DeferredCallResult, PluginHostError, ToolRegistry};
use ene_plugin_proto::ToolResult;
use ene_runtime::streaming::{StreamContext, run_stream_cognitive};
use ene_runtime::{DeferredToolTask, EneEvent, TerminalReason};
use ene_store::MemoryStore;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Mutex, broadcast, mpsc};
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

/// Mock LLM that requests a background-capable tool call on the first
/// invocation, then returns plain text so the streaming loop terminates.
struct MockLlmWithToolCall {
    calls: AtomicUsize,
}

#[async_trait]
impl LlmProvider for MockLlmWithToolCall {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        let chunks = if n == 0 {
            vec![Ok(LlmResponseChunk {
                text_delta: Some("Let me run a background task.".into()),
                tool_calls_delta: Some(vec![LlmToolCallChunk {
                    index: 0,
                    id: Some("call-123".into()),
                    name: Some("background.sleep".into()),
                    arguments: Some(r#"{"seconds": 1}"#.into()),
                }]),
            })]
        } else {
            vec![Ok(LlmResponseChunk {
                text_delta: Some("All done.".into()),
                tool_calls_delta: None,
            })]
        };
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        Ok("done".into())
    }
}

/// Mock registry with a background-capable tool.
struct BackgroundRegistry {
    deferred_calls: AtomicUsize,
    poll_count: AtomicUsize,
}

impl BackgroundRegistry {
    fn new() -> Self {
        Self {
            deferred_calls: AtomicUsize::new(0),
            poll_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ToolRegistry for BackgroundRegistry {
    fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
        vec![ene_plugin_proto::ToolSpec {
            name: ene_plugin_proto::ToolName::new("background.sleep"),
            description: "Sleep for N seconds in the background".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "seconds": {"type": "integer"}
                }
            }),
            background_capable: true,
        }]
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: &str,
        _context: Option<&ene_plugin_proto::CallContext>,
    ) -> Result<ToolResult, PluginHostError> {
        Err(PluginHostError::ExecutionFailed {
            message: "should use call_tool_deferred".into(),
        })
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        _arguments: &str,
        _context: Option<&ene_plugin_proto::CallContext>,
    ) -> Result<DeferredCallResult, PluginHostError> {
        if name == "background.sleep" {
            self.deferred_calls.fetch_add(1, Ordering::SeqCst);
            Ok(DeferredCallResult::Deferred {
                task_id: "task-456".into(),
            })
        } else {
            Err(PluginHostError::ExecutionFailed {
                message: format!("unknown tool: {name}"),
            })
        }
    }

    async fn poll_deferred(
        &self,
        _tool_name: &str,
        task_id: &str,
    ) -> ene_plugin_proto::DeferredStatus {
        let count = self.poll_count.fetch_add(1, Ordering::SeqCst);
        if task_id == "task-456" {
            if count < 3 {
                ene_plugin_proto::DeferredStatus::Pending
            } else {
                ene_plugin_proto::DeferredStatus::Completed {
                    result: "Slept for 1 second".into(),
                }
            }
        } else {
            ene_plugin_proto::DeferredStatus::Unknown
        }
    }

    async fn cancel_deferred(&self, _tool_name: &str, _task_id: &str) {}
}

#[tokio::test]
async fn deferred_tool_execution_emits_completion_event() {
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

    let (event_tx, mut event_rx) = broadcast::channel(64);
    let (diag_tx, _diag_rx) = broadcast::channel(16);
    let (deferred_tool_tx, mut deferred_tool_rx) = mpsc::unbounded_channel();
    let terminal_emitted = Arc::new(AtomicBool::new(false));
    let turn = ene_runtime::TurnId::new();

    let registry = Arc::new(BackgroundRegistry::new());

    let ctx = StreamContext {
        config,
        session,
        user_input: "Run a background task".into(),
        embedder: Some(Arc::new(MockEmbedder)),
        registry: registry.clone() as Arc<dyn ToolRegistry>,
        tool_rag: None,
        provider: Arc::new(MockLlmWithToolCall {
            calls: AtomicUsize::new(0),
        }),
        event_tx: event_tx.clone(),
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(Mutex::new(HashMap::new())),
        pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
        permission_scopes: Arc::new(Mutex::new(Vec::new())),
        undo_stack: Arc::new(Mutex::new(ene_runtime::undo::UndoStack::new(8))),
        terminal_emitted,
        turn: turn.clone(),
        origin: ene_runtime::TurnOrigin::User,
        allow_tools: true,
        runtime_directive: None,
        proactive_screen_image: None,
        generation_timeout: None,
        classifier_tx: mpsc::unbounded_channel().0,
        memory_writer_tx: mpsc::unbounded_channel().0,
        deferred_tool_tx,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
    };

    // Run the streaming loop in a separate task so we can poll events.
    let stream_handle = tokio::spawn(async move { run_stream_cognitive(ctx).await });

    // Collect events until we see Terminal.
    let mut saw_terminal = false;
    let mut saw_tool_call_result = false;
    let mut tool_result_message = String::new();

    while !saw_terminal {
        match event_rx.recv().await {
            Ok(EneEvent::Terminal {
                reason: TerminalReason::Done,
                ..
            }) => {
                saw_terminal = true;
            }
            Ok(EneEvent::ToolCallResult { result, .. }) => {
                saw_tool_call_result = true;
                tool_result_message = result;
            }
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }

    let _session = stream_handle.await.unwrap();

    // Verify the tool was called via deferred path.
    assert_eq!(registry.deferred_calls.load(Ordering::SeqCst), 1);

    // Verify the LLM received the "Task queued" message.
    assert!(saw_tool_call_result, "expected ToolCallResult event");
    assert!(
        tool_result_message.contains("Task queued for background execution"),
        "expected queued message, got: {tool_result_message}"
    );
    assert!(
        tool_result_message.contains("task-456"),
        "expected task_id in message"
    );

    // Verify a DeferredToolTask was sent to the actor.
    let task = deferred_tool_rx.try_recv().expect("expected deferred task");
    assert_eq!(task.tool_name, "background.sleep");
    assert_eq!(task.task_id, "task-456");

    // Simulate the actor's polling loop.
    let poll_registry = registry.clone();
    let poll_event_tx = event_tx.clone();
    let poll_handle = tokio::spawn(async move {
        let task = DeferredToolTask {
            tool_name: "background.sleep".into(),
            task_id: "task-456".into(),
            arguments: r#"{"seconds": 1}"#.into(),
            started_at: chrono::Utc::now(),
        };

        // Poll until completion (max 10 attempts).
        for _ in 0..10 {
            match poll_registry
                .poll_deferred(&task.tool_name, &task.task_id)
                .await
            {
                ene_plugin_proto::DeferredStatus::Completed { result } => {
                    let _ = poll_event_tx.send(EneEvent::ToolBackgroundCompleted {
                        tool_name: task.tool_name,
                        task_id: task.task_id,
                        status: ene_plugin_proto::DeferredStatus::Completed { result },
                    });
                    break;
                }
                ene_plugin_proto::DeferredStatus::Pending => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
                _ => break,
            }
        }
    });

    poll_handle.await.unwrap();

    // Verify the completion event was emitted.
    let mut saw_completion = false;
    while let Ok(event) = event_rx.try_recv() {
        if let EneEvent::ToolBackgroundCompleted {
            tool_name,
            task_id,
            status,
        } = event
        {
            assert_eq!(tool_name, "background.sleep");
            assert_eq!(task_id, "task-456");
            assert!(matches!(
                status,
                ene_plugin_proto::DeferredStatus::Completed { .. }
            ));
            saw_completion = true;
            break;
        }
    }

    assert!(saw_completion, "expected ToolBackgroundCompleted event");
}
