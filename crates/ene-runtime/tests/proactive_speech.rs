//! Proactive speech runtime integration tests (#169).

use ene_ai::{LlmMessage, LlmProvider, LlmProviderError, LlmResponseChunk, Role};
use ene_config::{CharacterCardV3, EneConfig};
use ene_mind::MindConfig;
use ene_runtime::streaming::{StreamContext, run_stream_cognitive};
use ene_runtime::{TurnId, TurnOrigin};
use ene_store::MemoryStore;
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

struct EchoProvider {
    last_messages: Mutex<Vec<LlmMessage>>,
    response: String,
}

#[async_trait]
impl LlmProvider for EchoProvider {
    fn name(&self) -> &str {
        "echo"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        *self.last_messages.lock().expect("lock") = messages.to_vec();
        let stream = tokio_stream::once(Ok(LlmResponseChunk {
            text_delta: Some(self.response.clone()),
            tool_calls_delta: None,
        }));
        Ok(Box::pin(stream))
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
impl ene_tool_host::ToolRegistry for EmptyRegistry {
    fn list_tools(&self) -> Vec<ene_tool_proto::ToolSpec> {
        Vec::new()
    }

    async fn call_tool(
        &self,
        _name: &str,
        _arguments: &str,
    ) -> Result<String, ene_tool_host::ToolHostError> {
        Err(ene_tool_host::ToolHostError::ExecutionFailed {
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
    let (event_tx, _event_rx) = tokio::sync::broadcast::channel(8);
    let (diag_tx, _diag_rx) = tokio::sync::broadcast::channel(8);
    let history_before = session.history().len();

    let ctx = StreamContext {
        config,
        session,
        user_input: String::new(),
        embedder: None,
        registry: Arc::new(EmptyRegistry) as Arc<dyn ene_tool_host::ToolRegistry>,
        tool_rag: None,
        provider,
        event_tx,
        diag_tx,
        cancel_token: CancellationToken::new(),
        pending_permissions: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        pending_user_inputs: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        terminal_emitted: Arc::new(AtomicBool::new(false)),
        turn: TurnId::new(),
        origin: TurnOrigin::Proactive,
        allow_tools: false,
        runtime_directive: Some("Speak briefly.".into()),
        generation_timeout: Some(std::time::Duration::from_secs(30)),
        classifier_tx: tokio::sync::mpsc::unbounded_channel().0,
    };

    let updated = run_stream_cognitive(ctx).await;
    assert_eq!(updated.history().len(), history_before + 1);
    assert!(
        !updated
            .history()
            .iter()
            .any(|e| e.role == Role::User),
        "proactive turn must not add synthetic user history"
    );
}

#[test]
fn proactive_generation_model_override_resolves() {
    let mut cfg = ene_ai::ProviderConfig::default();
    cfg.proactive.generation_model = "gpt-4o".into();
    assert_eq!(cfg.proactive_generation_model(), "gpt-4o");
}
