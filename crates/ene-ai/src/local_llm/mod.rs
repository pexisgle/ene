//! Local `llama-server` subprocess provider for proactive decisions (#165).
//!
//! Starts an OpenAI-compatible server on loopback only. Does not implement GPU
//! kernels; acceleration is delegated to the external binary.

mod process;
mod routing;

pub use process::{LlamaServerHandle, LlamaServerSpec, resolve_llama_server_executable};
pub use routing::{
    DecisionProviderKind, DisabledDecisionProvider, ProactiveLlmHandles,
    build_proactive_llm_handles,
};

use async_trait::async_trait;
use parking_lot::Mutex;
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;

use crate::error::LlmProviderError;
use crate::message::{LlmMessage, LlmResponseChunk};
use crate::openai::OpenAiProvider;
use crate::traits::LlmProvider;

/// OpenAI-compatible client backed by a managed local `llama-server`.
pub struct LocalLlamaCppProvider {
    inner: OpenAiProvider,
    server: Arc<Mutex<Option<LlamaServerHandle>>>,
}

impl LocalLlamaCppProvider {
    /// Start `llama-server` from `spec` and wrap it as an [`LlmProvider`].
    pub async fn start(spec: LlamaServerSpec) -> Result<Self, LlmProviderError> {
        let handle = LlamaServerHandle::spawn(spec).await?;
        let base = format!("http://127.0.0.1:{}", handle.port());
        // llama-server accepts any non-empty key (or none); use a placeholder.
        let inner = OpenAiProvider::new(&base, "local", "local")
            .with_chat_max_tokens(256)
            .with_thinking_disabled(true);
        Ok(Self {
            inner,
            server: Arc::new(Mutex::new(Some(handle))),
        })
    }

    /// Gracefully stop the child process (preferred over relying on `Drop`).
    pub async fn shutdown(&self) {
        let handle = { self.server.lock().take() };
        if let Some(handle) = handle {
            handle.shutdown().await;
        }
    }

    /// Loopback port the server listens on (for tests / diagnostics).
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.server.lock().as_ref().map(LlamaServerHandle::port)
    }
}

impl Drop for LocalLlamaCppProvider {
    fn drop(&mut self) {
        if let Some(handle) = self.server.lock().take() {
            handle.kill_blocking();
        }
    }
}

#[async_trait]
impl LlmProvider for LocalLlamaCppProvider {
    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "LlmProvider::name returns &str; static literals still satisfy the trait"
    )]
    fn name(&self) -> &str {
        "llama-cpp-local"
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        // Decision path must not use tools.
        if !tools.is_empty() {
            return Err(LlmProviderError::Provider(
                "local decision provider does not allow tool calls".to_string(),
            ));
        }
        self.inner.create_chat_stream(messages, &[]).await
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        self.inner.chat_completion(messages, json_schema).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ProactiveAcceleration, ProactiveDecisionBackend, ProactiveDecisionFallback,
        ProactiveDecisionProviderConfig, ProactiveProviderConfig, ProviderConfig,
    };
    use crate::local_llm::routing::build_proactive_llm_handles;

    #[tokio::test]
    async fn disabled_backend_returns_disabled_provider() {
        let provider_cfg = ProviderConfig {
            proactive: ProactiveProviderConfig {
                decision: ProactiveDecisionProviderConfig {
                    backend: ProactiveDecisionBackend::Disabled,
                    ..ProactiveDecisionProviderConfig::default()
                },
                generation_model: String::new(),
            },
            ..ProviderConfig::default()
        };
        let handles = build_proactive_llm_handles(&provider_cfg)
            .await
            .expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
        let out = handles
            .decision
            .chat_completion(&[], None)
            .await
            .expect("disabled returns silent json");
        assert!(out.contains("should_speak"));
        assert!(out.contains("false"));
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn missing_model_path_fails_closed_for_llama_cpp() {
        let provider_cfg = ProviderConfig {
            proactive: ProactiveProviderConfig {
                decision: ProactiveDecisionProviderConfig {
                    backend: ProactiveDecisionBackend::LlamaCpp,
                    model_path: String::new(),
                    executable: "/nonexistent/llama-server".into(),
                    acceleration: ProactiveAcceleration::Cpu,
                    fallback: ProactiveDecisionFallback::Disabled,
                    ..ProactiveDecisionProviderConfig::default()
                },
                generation_model: String::new(),
            },
            ..ProviderConfig::default()
        };
        let err = match build_proactive_llm_handles(&provider_cfg).await {
            Ok(_) => panic!("expected empty model path to fail"),
            Err(e) => e,
        };
        assert!(
            matches!(err, LlmProviderError::LocalProcess(_)),
            "got {err:?}"
        );
    }
}
