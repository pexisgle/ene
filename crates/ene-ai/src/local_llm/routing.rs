//! Decision provider routing for proactive speech (#165).

use super::{LlamaServerSpec, LocalLlamaCppProvider};
use crate::config::{
    ProactiveDecisionBackend, ProactiveDecisionFallback, ProactiveDecisionProviderConfig,
    ProviderConfig,
};
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, LlmResponseChunk};
use crate::openai::OpenAiProvider;
use crate::traits::LlmProvider;
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::Arc;
use tokio_stream::Stream;

/// Which decision backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionProviderKind {
    /// Local llama-server.
    LlamaCpp,
    /// Cloud OpenAI-compatible.
    Cloud,
    /// Always silent.
    Disabled,
}

/// Always returns a silent decision JSON (no network).
pub struct DisabledDecisionProvider;

#[async_trait]
impl LlmProvider for DisabledDecisionProvider {
    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "LlmProvider::name returns &str; static literals still satisfy the trait"
    )]
    fn name(&self) -> &str {
        "proactive-disabled"
    }

    async fn create_chat_stream(
        &self,
        _messages: &[LlmMessage],
        _tools: &[ene_tool_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        Err(LlmProviderError::Provider(
            "disabled decision provider does not stream".to_string(),
        ))
    }

    async fn chat_completion(
        &self,
        _messages: &[LlmMessage],
        _json_schema: Option<serde_json::Value>,
    ) -> Result<String, LlmProviderError> {
        Ok(
            r#"{"should_speak":false,"confidence":0.0,"reason":"decision backend disabled","topic_hint":"","urgency":"normal"}"#
                .to_string(),
        )
    }
}

/// Owned proactive LLM handles (decision + optional local process).
pub struct ProactiveLlmHandles {
    /// Provider used for structured decisions.
    pub decision: Arc<dyn LlmProvider>,
    /// Backend kind in use.
    pub decision_kind: DecisionProviderKind,
    /// Local server (if any) for explicit shutdown.
    local: Option<Arc<LocalLlamaCppProvider>>,
    /// Effective generation model name for proactive utterances.
    pub generation_model: String,
}

impl ProactiveLlmHandles {
    /// Shut down any local `llama-server` child.
    pub async fn shutdown(&self) {
        if let Some(local) = &self.local {
            local.shutdown().await;
        }
    }
}

/// Build decision + generation routing from [`ProviderConfig`].
///
/// Local failures respect `fallback`: `disabled` → error (caller fail-closes);
/// `cloud` → cloud decision provider when credentials allow.
pub async fn build_proactive_llm_handles(
    provider: &ProviderConfig,
) -> Result<ProactiveLlmHandles, LlmProviderError> {
    let generation_model = provider.proactive_generation_model().to_string();
    let decision_cfg = &provider.proactive.decision;

    match decision_cfg.backend {
        ProactiveDecisionBackend::Disabled => Ok(ProactiveLlmHandles {
            decision: Arc::new(DisabledDecisionProvider),
            decision_kind: DecisionProviderKind::Disabled,
            local: None,
            generation_model,
        }),
        ProactiveDecisionBackend::Cloud => {
            let cloud = build_cloud_decision_provider(provider, decision_cfg)?;
            Ok(ProactiveLlmHandles {
                decision: cloud,
                decision_kind: DecisionProviderKind::Cloud,
                local: None,
                generation_model,
            })
        }
        ProactiveDecisionBackend::LlamaCpp => match start_local(decision_cfg).await {
            Ok(local) => {
                let arc = Arc::new(local);
                let decision: Arc<dyn LlmProvider> = arc.clone();
                Ok(ProactiveLlmHandles {
                    decision,
                    decision_kind: DecisionProviderKind::LlamaCpp,
                    local: Some(arc),
                    generation_model,
                })
            }
            Err(e) => match decision_cfg.fallback {
                ProactiveDecisionFallback::Disabled => Err(e),
                ProactiveDecisionFallback::Cloud => {
                    tracing::warn!(
                        component = "LocalLlamaCpp",
                        error = %e,
                        "Local decision backend failed; falling back to cloud"
                    );
                    let cloud = build_cloud_decision_provider(provider, decision_cfg)?;
                    Ok(ProactiveLlmHandles {
                        decision: cloud,
                        decision_kind: DecisionProviderKind::Cloud,
                        local: None,
                        generation_model,
                    })
                }
            },
        },
    }
}

async fn start_local(
    decision_cfg: &ProactiveDecisionProviderConfig,
) -> Result<LocalLlamaCppProvider, LlmProviderError> {
    let spec = LlamaServerSpec::from_config(decision_cfg)?;
    LocalLlamaCppProvider::start(spec).await
}

fn build_cloud_decision_provider(
    provider: &ProviderConfig,
    decision_cfg: &ProactiveDecisionProviderConfig,
) -> Result<Arc<dyn LlmProvider>, LlmProviderError> {
    let base_url = provider
        .resolve_base_url()
        .map_err(|e| LlmProviderError::Provider(e.to_string()))?;
    let api_key = provider.resolve_api_key();
    let model = if decision_cfg.cloud_model.trim().is_empty() {
        provider.model.as_str()
    } else {
        decision_cfg.cloud_model.trim()
    };
    let cloud = OpenAiProvider::new(&base_url, &api_key, model)
        .with_chat_max_tokens(256)
        .with_thinking_disabled(true);
    Ok(Arc::new(cloud))
}

/// Optional env-gated smoke: only runs when `ENE_LOCAL_LLM_BIN` and
/// `ENE_LOCAL_LLM_MODEL` are set.
#[cfg(test)]
mod smoke {
    use super::*;
    use crate::config::{
        ProactiveAcceleration, ProactiveDecisionBackend, ProactiveDecisionFallback,
        ProactiveDecisionProviderConfig, ProactiveProviderConfig, ProviderConfig,
    };
    use crate::message::UserMessagePart;

    fn env_smoke_enabled() -> Option<(String, String, ProactiveAcceleration)> {
        let bin = std::env::var("ENE_LOCAL_LLM_BIN").ok()?;
        let model = std::env::var("ENE_LOCAL_LLM_MODEL").ok()?;
        if bin.trim().is_empty() || model.trim().is_empty() {
            return None;
        }
        let accel = match std::env::var("ENE_LOCAL_LLM_BACKEND")
            .unwrap_or_else(|_| "cpu".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "vulkan" => ProactiveAcceleration::Vulkan,
            "cuda" => ProactiveAcceleration::Cuda,
            _ => ProactiveAcceleration::Cpu,
        };
        Some((bin, model, accel))
    }

    #[tokio::test]
    async fn optional_local_llama_smoke() {
        let Some((bin, model, accel)) = env_smoke_enabled() else {
            return;
        };
        let provider_cfg = ProviderConfig {
            proactive: ProactiveProviderConfig {
                decision: ProactiveDecisionProviderConfig {
                    backend: ProactiveDecisionBackend::LlamaCpp,
                    model_path: model,
                    executable: bin,
                    acceleration: accel,
                    fallback: ProactiveDecisionFallback::Disabled,
                    context_size: 1024,
                    startup_timeout_seconds: 120,
                    request_timeout_seconds: 30,
                    ..ProactiveDecisionProviderConfig::default()
                },
                generation_model: String::new(),
            },
            ..ProviderConfig::default()
        };
        let handles = build_proactive_llm_handles(&provider_cfg)
            .await
            .expect("local llama smoke start");
        assert_eq!(handles.decision_kind, DecisionProviderKind::LlamaCpp);
        let messages = vec![
            LlmMessage::System {
                content:
                    "Return JSON only with should_speak, confidence, reason, topic_hint, urgency."
                        .into(),
            },
            LlmMessage::User {
                parts: vec![UserMessagePart::Text {
                    text: "seconds_since_user_input: 400\nactivity: idle".into(),
                }],
            },
        ];
        let out = handles
            .decision
            .chat_completion(&messages, None)
            .await
            .expect("decision completion");
        assert!(
            out.contains("should_speak"),
            "expected decision json, got: {out}"
        );
        handles.shutdown().await;
    }
}
