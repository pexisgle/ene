//! Decision provider routing for proactive speech (#165 / #171).

use super::{LocalGgufLoadParams, LocalLlamaCppProvider};
use crate::config::{AiConfig, AiProviderDef};
use crate::error::LlmProviderError;
use crate::message::{LlmMessage, LlmResponseChunk};
use crate::openai::OpenAiProvider;
use crate::resolve::ResolvedChat;
use crate::traits::LlmProvider;
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;

const LOCAL_STARTUP_TIMEOUT_SECS: u64 = 60;
const LOCAL_REQUEST_TIMEOUT_SECS: u64 = 20;
const DECISION_MAX_TOKENS: u32 = 256;

/// Which decision backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionProviderKind {
    /// In-process llama-cpp-2.
    LlamaCpp,
    /// Cloud OpenAI-compatible.
    Cloud,
    /// Always silent (local load failed with disabled fallback).
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

/// Owned proactive LLM handles (decision + optional local model).
pub struct ProactiveLlmHandles {
    /// Provider used for structured decisions.
    pub decision: Arc<dyn LlmProvider>,
    /// Backend kind in use.
    pub decision_kind: DecisionProviderKind,
    /// Local model (if any) for explicit shutdown.
    local: Option<Arc<LocalLlamaCppProvider>>,
    /// Effective generation model name for proactive utterances.
    pub generation_model: String,
}

impl ProactiveLlmHandles {
    /// Shut down any local resources (model drop is sufficient; kept for API stability).
    pub async fn shutdown(&self) {
        if let Some(local) = &self.local {
            local.shutdown().await;
        }
    }
}

/// Build decision + generation routing from [`AiConfig`].
///
/// Local GGUF load failures fall back to a cloud decision provider when an
/// OpenAI-compatible chat/proactive task is available; otherwise fail-closed
/// to [`DisabledDecisionProvider`].
pub async fn build_proactive_llm_handles(
    config: &AiConfig,
) -> Result<ProactiveLlmHandles, LlmProviderError> {
    let generation = config.resolve_proactive_generation()?;
    let generation_model = generation.model.clone();

    let decision_provider = decision_provider_def(config)?;
    match decision_provider {
        AiProviderDef::LocalGguf {
            model_path,
            acceleration,
            gpu_layers,
            context_size,
            ..
        } if !model_path.trim().is_empty() => {
            let params = LocalGgufLoadParams {
                model_path: model_path.clone(),
                acceleration: *acceleration,
                gpu_layers: gpu_layers.clone(),
                context_size: *context_size,
                request_timeout_seconds: LOCAL_REQUEST_TIMEOUT_SECS,
            };
            match start_local(&params).await {
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
                Err(e) => {
                    if let Ok(cloud) = build_cloud_decision_provider(config) {
                        tracing::warn!(
                            component = "LocalLlamaCpp",
                            error = %e,
                            "Local decision backend failed; falling back to cloud"
                        );
                        Ok(ProactiveLlmHandles {
                            decision: cloud,
                            decision_kind: DecisionProviderKind::Cloud,
                            local: None,
                            generation_model,
                        })
                    } else {
                        tracing::warn!(
                            component = "LocalLlamaCpp",
                            error = %e,
                            "Local decision backend failed; falling back to disabled"
                        );
                        Ok(ProactiveLlmHandles {
                            decision: Arc::new(DisabledDecisionProvider),
                            decision_kind: DecisionProviderKind::Disabled,
                            local: None,
                            generation_model,
                        })
                    }
                }
            }
        }
        _ => {
            let cloud = build_cloud_decision_provider(config)?;
            Ok(ProactiveLlmHandles {
                decision: cloud,
                decision_kind: DecisionProviderKind::Cloud,
                local: None,
                generation_model,
            })
        }
    }
}

fn decision_provider_def(config: &AiConfig) -> Result<&AiProviderDef, LlmProviderError> {
    if let Some(proactive) = config.tasks.proactive.as_ref()
        && let Ok(provider) = config.get_provider(&proactive.provider)
    {
        return Ok(provider);
    }
    config.get_provider(&config.tasks.chat.provider)
}

async fn start_local(
    params: &LocalGgufLoadParams,
) -> Result<LocalLlamaCppProvider, LlmProviderError> {
    let cfg = params.clone();
    let load_timeout = Duration::from_secs(LOCAL_STARTUP_TIMEOUT_SECS.max(1));
    let join = tokio::task::spawn_blocking(move || LocalLlamaCppProvider::load(&cfg));
    match tokio::time::timeout(load_timeout, join).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(LlmProviderError::LocalLlm(format!(
            "local model load join error: {e}"
        ))),
        Err(_) => Err(LlmProviderError::LocalLlm(format!(
            "local model load timed out after {load_timeout:?}"
        ))),
    }
}

fn build_cloud_decision_provider(
    config: &AiConfig,
) -> Result<Arc<dyn LlmProvider>, LlmProviderError> {
    let resolved = match config.tasks.proactive.as_ref() {
        Some(proactive)
            if matches!(
                config.get_provider(&proactive.provider).ok(),
                Some(AiProviderDef::OpenaiCompatible { .. })
            ) =>
        {
            config.resolve_chat_task(Some(proactive))?
        }
        _ => config.resolve_chat()?,
    };
    Ok(Arc::new(cloud_decision_from_resolved(&resolved)))
}

fn cloud_decision_from_resolved(chat: &ResolvedChat) -> OpenAiProvider {
    OpenAiProvider::new(&chat.base_url, &chat.api_key, &chat.model)
        .with_chat_max_tokens(DECISION_MAX_TOKENS)
        .with_thinking_disabled(true)
}

/// Optional env-gated smoke: only runs when `ENE_LOCAL_LLM_MODEL` is set.
#[cfg(test)]
mod smoke {
    use super::*;
    use crate::config::{AiConfig, AiProviderDef, ProactiveAcceleration, TaskRef};
    use crate::message::UserMessagePart;

    fn env_smoke_enabled() -> Option<(String, ProactiveAcceleration)> {
        let model = std::env::var("ENE_LOCAL_LLM_MODEL").ok()?;
        if model.trim().is_empty() {
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
        Some((model, accel))
    }

    #[tokio::test]
    async fn optional_local_llama_smoke() {
        let Some((model, accel)) = env_smoke_enabled() else {
            return;
        };
        let mut cfg = AiConfig::default();
        cfg.tasks.proactive = Some(TaskRef {
            provider: "local".to_string(),
            model: None,
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
        });
        cfg.providers.insert(
            "local".to_string(),
            AiProviderDef::LocalGguf {
                model: String::new(),
                quantization: "F16".to_string(),
                model_path: model,
                acceleration: accel,
                gpu_layers: "auto".to_string(),
                context_size: 1024,
            },
        );
        let handles = build_proactive_llm_handles(&cfg)
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
