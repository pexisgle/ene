//! Proactive speech runtime integration tests.

#![expect(
    clippy::expect_used,
    clippy::field_reassign_with_default,
    reason = "integration tests use expect and default-then-assign fixtures"
)]

use async_trait::async_trait;
use ene_ai::{LlmCompletion, LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk, Role};
use ene_config::{CharacterCardV3, EneConfig};
use ene_mind::MindConfig;
use ene_plugin_proto::ToolResult;
use ene_runtime::streaming::{StreamContext, run_stream_cognitive};
use ene_runtime::{TerminalReason, TurnId, TurnOrigin};
use ene_store::MemoryStore;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

/// Stub host that serves no providers; proactive turns supply their own
/// provider and never trigger the classifier path.
struct EmptyProviderHost;

#[async_trait]
impl ene_ai::ProviderHost for EmptyProviderHost {
    async fn create_llm_provider(
        &self,
        kind: &str,
        _config: &ene_config::EneConfig,
        _task: &ene_ai::config::TaskRef,
    ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
        Err(LlmProviderError::Provider(format!(
            "No LlmProviderFactory registered for provider kind: '{kind}'"
        )))
    }

    async fn create_embedding_provider(
        &self,
        _kind: &str,
        _config: &ene_config::EneConfig,
    ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
        Err(ene_ai::EmbeddingError::Init(
            "stub host serves no embedding providers".to_string(),
        ))
    }

    async fn create_tts_provider(
        &self,
        _kind: &str,
        _config: &ene_config::EneConfig,
    ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
        Err(ene_ai::AudioProviderError::Provider(
            "stub host serves no TTS providers".to_string(),
        ))
    }
}

struct EchoProvider {
    last_messages: Mutex<Vec<LlmMessage>>,
    response: String,
}

struct ChunkedProvider {
    chunks: Vec<String>,
    last_messages: Mutex<Vec<LlmMessage>>,
}

#[async_trait]
impl LlmProvider for EchoProvider {
    fn name(&self) -> &'static str {
        "echo"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        _tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        *self.last_messages.lock().expect("lock") = messages.to_vec();
        let stream = tokio_stream::once(Ok(LlmResponseChunk {
            text_delta: Some(self.response.clone()),
            tool_calls_delta: None,
            usage: None,
        }));
        Ok(Box::pin(stream))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        Ok(LlmCompletion::text_only(self.response.clone()))
    }
}

#[async_trait]
impl LlmProvider for ChunkedProvider {
    fn name(&self) -> &'static str {
        "chunked"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        _tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        *self.last_messages.lock().expect("lock") = messages.to_vec();
        let stream = tokio_stream::iter(self.chunks.clone().into_iter().map(|text_delta| {
            Ok(LlmResponseChunk {
                text_delta: Some(text_delta),
                tool_calls_delta: None,
                usage: None,
            })
        }));
        Ok(Box::pin(stream))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        Ok(LlmCompletion::text_only(String::new()))
    }
}

struct EmptyRegistry;

#[async_trait]
impl ene_plugin_host::ToolRegistry for EmptyRegistry {
    fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
        Vec::new()
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: &str,
        _context: Option<&ene_plugin_proto::CallContext>,
    ) -> Result<ToolResult, ene_plugin_host::PluginHostError> {
        Err(ene_plugin_host::PluginHostError::ExecutionFailed {
            message: "not used".into(),
        })
    }
}

#[tokio::test]
async fn proactive_stream_does_not_add_user_history() {
    let store = Arc::new(MemoryStore::open_in_memory(4).await.expect("store"));
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);
    session.memory.memory_store = Some(store);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    config.set_section(&mind).expect("mind");
    let mut mem_config = ene_store::StoreConfig::default();
    mem_config.enabled = true;
    config.set_section(&mem_config).expect("memory");

    let provider = Arc::new(EchoProvider {
        last_messages: Mutex::new(Vec::new()),
        response: "Hey there!".into(),
    });
    let provider_for_assert = Arc::clone(&provider);
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);
    let history_before = session.history().len();

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx,
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Speak briefly.".into()),
        proactive_screen_image: None,
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(outcome.terminal, TerminalReason::Done);
    let updated = outcome.session;
    assert_eq!(updated.history().len(), history_before + 1);
    assert!(
        !updated.history().iter().any(|e| e.role == Role::User),
        "proactive turn must not add synthetic user history"
    );
    let messages = provider_for_assert.last_messages.lock().expect("lock");
    let has_synthetic_user = messages.iter().any(|m| {
        let LlmMessage::User { parts } = m else {
            return false;
        };
        parts.iter().any(
            |p| matches!(p, ene_ai::UserMessagePart::Text { text } if text.contains("unprompted")),
        )
    });
    assert!(
        !has_synthetic_user,
        "proactive directive must not inject synthetic user messages"
    );
    let has_image = messages.iter().any(|m| {
        let LlmMessage::User { parts } = m else {
            return false;
        };
        parts
            .iter()
            .any(|p| matches!(p, ene_ai::UserMessagePart::Image { .. }))
    });
    assert!(!has_image, "no screen image was provided for this turn");
}

#[tokio::test]
async fn proactive_stream_attaches_screen_image_when_provided() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    config.set_section(&mind).expect("mind");

    let provider = Arc::new(EchoProvider {
        last_messages: Mutex::new(Vec::new()),
        response: "I see your screen!".into(),
    });
    let provider_for_assert = Arc::clone(&provider);
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx,
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("React to the screen.".into()),
        proactive_screen_image: Some("data:image/jpeg;base64,AAAA".into()),
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(outcome.terminal, TerminalReason::Done);
    let messages = provider_for_assert.last_messages.lock().expect("lock");
    let image_uri = messages.iter().find_map(|m| {
        let LlmMessage::User { parts } = m else {
            return None;
        };
        parts.iter().find_map(|p| match p {
            ene_ai::UserMessagePart::Image { base64_image_data } => {
                Some(base64_image_data.as_str())
            }
            ene_ai::UserMessagePart::Text { .. } => None,
        })
    });
    assert_eq!(image_uri, Some("data:image/jpeg;base64,AAAA"));
}

#[tokio::test]
async fn proactive_stream_without_memory_store() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    config.set_section(&mind).expect("mind");

    let provider = Arc::new(EchoProvider {
        last_messages: Mutex::new(Vec::new()),
        response: "Hi!".into(),
    });
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx,
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Check in briefly.".into()),
        proactive_screen_image: None,
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(outcome.terminal, TerminalReason::Done);
    assert_eq!(outcome.session.history().len(), 1);
}

#[tokio::test]
async fn proactive_stream_declines_on_leading_silent_token() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    mind.proactive.confirmation_enabled = true;
    config.set_section(&mind).expect("mind");

    let provider = Arc::new(ChunkedProvider {
        chunks: vec!["<|silent|>".into(), "SHOULD NOT APPEAR".into()],
        last_messages: Mutex::new(Vec::new()),
    });
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);
    let partial_text = Arc::new(parking_lot::Mutex::new(String::new()));
    let partial_for_assert = Arc::clone(&partial_text);

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx: event_tx.clone(),
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Speak briefly.".into()),
        proactive_screen_image: None,
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text,
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(outcome.terminal, TerminalReason::Declined);
    assert!(!outcome.spoke_visible_text);
    assert_eq!(
        outcome.session.history().len(),
        0,
        "a declined turn must not commit history"
    );
    assert!(
        partial_for_assert.lock().is_empty(),
        "no text may survive a declined turn"
    );

    let mut terminal_seen = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            ene_runtime::EneEvent::TextDelta { delta, .. } => {
                assert!(
                    delta.trim().is_empty(),
                    "no visible text may reach consumers, got {delta:?}"
                );
            }
            ene_runtime::EneEvent::Terminal { reason, .. } => {
                assert_eq!(reason, TerminalReason::Declined);
                terminal_seen = true;
            }
            _ => {}
        }
    }
    assert!(terminal_seen, "a Declined terminal event must be emitted");
}

#[tokio::test]
async fn proactive_stream_speaks_when_text_precedes_silent_token() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    mind.proactive.confirmation_enabled = true;
    config.set_section(&mind).expect("mind");

    let provider = Arc::new(ChunkedProvider {
        chunks: vec!["Hello <|silent|> there".into()],
        last_messages: Mutex::new(Vec::new()),
    });
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx,
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Speak briefly.".into()),
        proactive_screen_image: None,
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(outcome.terminal, TerminalReason::Done);
    assert!(outcome.spoke_visible_text);
    let updated = outcome.session;
    assert_eq!(updated.history().len(), 1);
    assert_eq!(
        updated.history()[0].content,
        "Hello  there",
        "a late refusal token must be stripped, not spoken"
    );
}

#[tokio::test]
async fn proactive_stream_ignores_silent_token_without_confirmation() {
    let mut card = CharacterCardV3::default();
    card.data.name = "Ene".into();
    card.data.system_prompt = "Be helpful.".into();

    let mut session = ene_mind::ConversationSession::new();
    session.character_card = Some(card);

    let mut config = EneConfig::default();
    let mut mind = MindConfig::default();
    mind.proactive.enabled = true;
    config.set_section(&mind).expect("mind");

    let provider = Arc::new(ChunkedProvider {
        chunks: vec!["<|silent|>".into()],
        last_messages: Mutex::new(Vec::new()),
    });
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_plugin_host::ToolRegistry>,
        tool_rag: None,
        provider,
        provider_host: Arc::new(EmptyProviderHost),
        event_tx,
        audio_tx: tokio::sync::mpsc::channel(8).0,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        permission_scopes: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        undo_stack: Arc::new(tokio::sync::Mutex::new(ene_runtime::undo::UndoStack::new(
            8,
        ))),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Speak briefly.".into()),
        proactive_screen_image: None,
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        proactive_topic: None,
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
        memory_writer_tx: tokio::sync::mpsc::unbounded_channel().0,
        deferred_tool_tx: tokio::sync::mpsc::unbounded_channel().0,
        aux_task_tx: tokio::sync::mpsc::unbounded_channel().0,
        tts_provider: None,
        partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
        compression_pending: false,
        concrete_store: None,
    };

    let outcome = run_stream_cognitive(ctx).await;
    assert_eq!(
        outcome.terminal,
        TerminalReason::Done,
        "without confirmation the token is an ordinary marker"
    );
    assert!(!outcome.spoke_visible_text);
    assert_eq!(outcome.session.history().len(), 1);
}

#[test]
fn proactive_generation_task_override_resolves() {
    let mut cfg = ene_ai::AiConfig::default();
    if let Some(def) = cfg.providers.get_mut("default") {
        def.base_url = "https://api.openai.com/v1".to_string();
    }
    cfg.tasks.proactive = Some(ene_ai::TaskRef {
        provider: "default".to_string(),
        model: Some("gpt-4o".to_string()),
        ..Default::default()
    });
    let resolved = cfg.resolve_proactive_generation().expect("resolve");
    assert_eq!(resolved.model, "gpt-4o");
}
