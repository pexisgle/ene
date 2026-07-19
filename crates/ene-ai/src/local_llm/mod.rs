//! In-process llama-cpp-2 provider for proactive decisions (#165 / #171).

mod routing;

pub use routing::{
    DecisionProviderKind, DisabledDecisionProvider, ProactiveLlmHandles,
    build_proactive_llm_handles,
};

use crate::config::ProactiveAcceleration;
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

/// Parameters for loading a local GGUF decision model.
#[derive(Debug, Clone)]
pub struct LocalGgufLoadParams {
    /// Absolute or relative path to GGUF weights.
    pub model_path: String,
    /// Optional path to multimodal projector GGUF (vision).
    pub mmproj_path: Option<String>,
    /// Preferred acceleration backend.
    pub acceleration: ProactiveAcceleration,
    /// GPU layer offload: `"auto"` or an integer string.
    pub gpu_layers: String,
    /// Context size for inference.
    pub context_size: u32,
    /// Per-request timeout for decision completion.
    pub request_timeout_seconds: u64,
}

/// Local decision provider backed by an in-process llama.cpp model.
pub struct LocalLlamaCppProvider {
    model: Arc<Mutex<LoadedModel>>,
    request_timeout: Duration,
}

impl LocalLlamaCppProvider {
    /// Load GGUF weights from `params` and wrap as an [`LlmProvider`].
    pub fn load(params: &LocalGgufLoadParams) -> Result<Self, LlmProviderError> {
        let model_path = LoadSpec::validate_model_path(&params.model_path)?;
        let mmproj_path = params
            .mmproj_path
            .as_ref()
            .map(|p| LoadSpec::validate_model_path(p))
            .transpose()?;
        let spec = LoadSpec {
            model_path,
            mmproj_path,
            acceleration: params.acceleration,
            gpu_layers: params.gpu_layers.clone(),
            context_size: params.context_size.max(256),
        };
        let loaded = LoadedModel::load(&spec)?;
        Ok(Self {
            model: Arc::new(Mutex::new(loaded)),
            request_timeout: Duration::from_secs(params.request_timeout_seconds.max(1)),
        })
    }

    /// True when an mmproj was loaded and reports vision support.
    #[must_use]
    pub fn supports_vision(&self) -> bool {
        self.model.lock().supports_vision()
    }

    /// Summarize an RGB8 screen capture with the local vision model.
    pub async fn summarize_rgb(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        system: &str,
        user: &str,
    ) -> Result<String, LlmProviderError> {
        let model = Arc::clone(&self.model);
        let timeout = self.request_timeout;
        let system = system.to_string();
        let user = user.to_string();
        tokio::task::spawn_blocking(move || {
            let guard = model.lock();
            crate::llama_cpp::generate_with_rgb_image(
                &guard, &system, &user, width, height, &rgb, timeout,
            )
        })
        .await
        .map_err(|e| LlmProviderError::LocalLlm(format!("vision task join error: {e}")))?
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
    use crate::config::{AiConfig, LOCAL_PROVIDER, LocalModelDef, ProactiveAcceleration, TaskRef};
    use crate::local_llm::routing::build_proactive_llm_handles;

    fn test_config() -> AiConfig {
        let mut cfg = AiConfig::default();
        if let Some(crate::config::AiProviderDef::OpenaiCompatible { base_url, .. }) =
            cfg.providers.get_mut("default")
        {
            *base_url = "https://api.openai.com/v1".to_string();
        }
        cfg
    }

    #[tokio::test]
    async fn cloud_provider_returns_cloud_decision() {
        let cfg = test_config();
        let handles = build_proactive_llm_handles(&cfg).await.expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
        assert_eq!(handles.generation_model, "gpt-4o-mini");
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn proactive_openai_task_drives_generation_model() {
        let mut cfg = test_config();
        cfg.tasks.proactive = Some(TaskRef {
            provider: "default".to_string(),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        let handles = build_proactive_llm_handles(&cfg).await.expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
        assert_eq!(handles.generation_model, "gpt-4o");
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn local_missing_weights_fails_closed_to_disabled() {
        let mut cfg = test_config();
        cfg.local_models.insert(
            "missing".to_string(),
            LocalModelDef {
                model_path: "/nonexistent/ene-missing-decision.gguf".to_string(),
                acceleration: ProactiveAcceleration::Cpu,
                ..LocalModelDef::default()
            },
        );
        cfg.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("missing".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        let handles = build_proactive_llm_handles(&cfg)
            .await
            .expect("missing weights fail-closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn missing_model_path_fails_closed_to_disabled() {
        let mut cfg = test_config();
        cfg.local_models.insert(
            "empty".to_string(),
            LocalModelDef {
                acceleration: ProactiveAcceleration::Cpu,
                ..LocalModelDef::default()
            },
        );
        cfg.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("empty".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        let handles = build_proactive_llm_handles(&cfg)
            .await
            .expect("empty model_path fail-closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn local_load_rejects_empty_model_path() {
        let params = LocalGgufLoadParams {
            model_path: String::new(),
            mmproj_path: None,
            acceleration: ProactiveAcceleration::Cpu,
            gpu_layers: "auto".to_string(),
            context_size: 2048,
            request_timeout_seconds: 20,
        };
        let err = match LocalLlamaCppProvider::load(&params) {
            Ok(_) => panic!("expected empty path to fail"),
            Err(e) => e,
        };
        assert!(matches!(err, LlmProviderError::LocalLlm(_)));
    }
}
