//! In-process llama-cpp-2 provider for proactive decisions (#165 / #171).

mod routing;

pub use routing::{
    DecisionProviderKind, DisabledDecisionProvider, ProactiveLlmHandles,
    build_proactive_llm_handles,
};

use crate::config::ProactiveDecisionProviderConfig;
use crate::error::LlmProviderError;
use crate::llama_cpp::{LoadSpec, LoadedModel, generate_chat};
use crate::message::{LlmMessage, LlmResponseChunk};
use crate::traits::LlmProvider;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;

/// Local decision provider backed by an in-process llama.cpp model.
pub struct LocalLlamaCppProvider {
    model: Arc<Mutex<LoadedModel>>,
    request_timeout: Duration,
}

impl LocalLlamaCppProvider {
    /// Load GGUF weights from `cfg` and wrap as an [`LlmProvider`].
    pub fn load(cfg: &ProactiveDecisionProviderConfig) -> Result<Self, LlmProviderError> {
        let model_path = LoadSpec::validate_model_path(&cfg.model_path)?;
        let spec = LoadSpec {
            model_path,
            acceleration: cfg.acceleration,
            gpu_layers: cfg.gpu_layers.clone(),
            context_size: cfg.context_size.max(256),
        };
        let loaded = LoadedModel::load(&spec)?;
        Ok(Self {
            model: Arc::new(Mutex::new(loaded)),
            request_timeout: Duration::from_secs(cfg.request_timeout_seconds.max(1)),
        })
    }

    /// No-op for API compatibility with the former subprocess provider.
    #[expect(
        clippy::unused_async,
        reason = "async kept for ProactiveLlmHandles::shutdown API"
    )]
    pub async fn shutdown(&self) {}
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
        if !tools.is_empty() {
            return Err(LlmProviderError::Provider(
                "local decision provider does not allow tool calls".to_string(),
            ));
        }
        let text = self.chat_completion(messages, None).await?;
        let stream = tokio_stream::once(Ok(LlmResponseChunk {
            text_delta: Some(text),
            tool_calls_delta: None,
        }));
        Ok(Box::pin(stream))
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        let messages = messages.to_vec();
        let schema = json_schema;
        let model = Arc::clone(&self.model);
        let timeout = self.request_timeout;
        tokio::task::spawn_blocking(move || {
            let guard = model.lock();
            generate_chat(&guard, &messages, schema.as_ref(), timeout)
        })
        .await
        .map_err(|e| LlmProviderError::LocalLlm(format!("decision task join error: {e}")))?
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
        assert!(matches!(err, LlmProviderError::LocalLlm(_)), "got {err:?}");
    }
}
