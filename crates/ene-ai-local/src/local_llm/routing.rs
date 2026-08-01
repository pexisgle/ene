//! Decision provider routing for proactive speech.

use super::{LocalGgufLoadParams, LocalLlamaCppProvider};
use crate::gguf::{ensure_gguf_available, ensure_mmproj_available};
use async_trait::async_trait;
use ene_ai::config::{AiConfig, AiProviderDef};
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmCompletion, LlmMessage, LlmResponseChunk};
use ene_ai::openai::OpenAiProvider;
use ene_ai::resolve::ResolvedChat;
use ene_ai::traits::LlmProvider;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;

const LOCAL_STARTUP_TIMEOUT_SECS: u64 = 300;
const LOCAL_REQUEST_TIMEOUT_SECS: u64 = 20;
const DECISION_MAX_TOKENS: u32 = 256;

/// Which decision backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionProviderKind {
    /// In-process llama-cpp-4.
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
        _tools: &[ene_plugin_proto::ToolSpec],
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
    ) -> Result<LlmCompletion, LlmProviderError> {
        Ok(LlmCompletion::text_only(
            r#"{"should_speak":false,"confidence":0.0,"reason":"decision backend disabled","topic_hint":"","urgency":"normal"}"#
                .to_string(),
        ))
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
    /// Local llama.cpp provider when the decision backend is in-process.
    #[must_use]
    pub fn local(&self) -> Option<&Arc<LocalLlamaCppProvider>> {
        self.local.as_ref()
    }

    /// Shut down any local resources (model drop is sufficient; kept for API stability).
    pub async fn shutdown(&self) {
        if let Some(local) = &self.local {
            local.shutdown().await;
        }
    }
}

/// Build decision + generation routing from [`AiConfig`].
///
/// When `tasks.proactive` requests `provider: "local"`, GGUF load failures
/// fail-closed to [`DisabledDecisionProvider`] — never silently route
/// observation context to a cloud decision provider.
pub async fn build_proactive_llm_handles(
    config: &AiConfig,
) -> Result<ProactiveLlmHandles, LlmProviderError> {
    let generation = config.resolve_proactive_generation()?;
    let generation_model = generation.model.clone();

    if let Some(proactive) = config.tasks.proactive.as_ref()
        && AiConfig::is_local_provider(&proactive.provider)
    {
        let local = config.resolve_local_model_for_task(proactive)?;
        let resolved_path = match ensure_gguf_available(&local).await {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(
                    component = "LocalLlamaCpp",
                    error = %e,
                    model = %local.name,
                    "Local GGUF unavailable; falling back to disabled (fail-closed)"
                );
                return Ok(disabled_handles(generation_model));
            }
        };
        let mmproj_path = match ensure_mmproj_available(&local).await {
            Ok(path) => path,
            Err(e) => {
                tracing::warn!(
                    component = "LocalLlamaCpp",
                    error = %e,
                    "mmproj unavailable; continuing text-only"
                );
                None
            }
        };
        let params = LocalGgufLoadParams {
            model_path: resolved_path.to_string_lossy().into_owned(),
            mmproj_path: mmproj_path.map(|p| p.to_string_lossy().into_owned()),
            acceleration: local.acceleration,
            gpu_layers: local.gpu_layers.clone(),
            context_size: local.context_size,
            request_timeout_seconds: LOCAL_REQUEST_TIMEOUT_SECS,
        };
        match start_local(&params).await {
            Ok(local_provider) => {
                let arc = Arc::new(local_provider);
                let decision: Arc<dyn LlmProvider> = arc.clone();
                Ok(ProactiveLlmHandles {
                    decision,
                    decision_kind: DecisionProviderKind::LlamaCpp,
                    local: Some(arc),
                    generation_model,
                })
            }
            Err(e) => {
                tracing::warn!(
                    component = "LocalLlamaCpp",
                    error = %e,
                    "Local decision backend failed; falling back to disabled (fail-closed)"
                );
                Ok(disabled_handles(generation_model))
            }
        }
    } else {
        let cloud = build_cloud_decision_provider(config)?;
        Ok(ProactiveLlmHandles {
            decision: cloud,
            decision_kind: DecisionProviderKind::Cloud,
            local: None,
            generation_model,
        })
    }
}

fn disabled_handles(generation_model: String) -> ProactiveLlmHandles {
    ProactiveLlmHandles {
        decision: Arc::new(DisabledDecisionProvider),
        decision_kind: DecisionProviderKind::Disabled,
        local: None,
        generation_model,
    }
}

async fn start_local(
    params: &LocalGgufLoadParams,
) -> Result<LocalLlamaCppProvider, LlmProviderError> {
    let cfg = params.clone();
    let load_timeout = Duration::from_secs(LOCAL_STARTUP_TIMEOUT_SECS.max(1));
    let (tx, rx) = tokio::sync::oneshot::channel();
    // A dedicated `std::thread`, not `tokio::task::spawn_blocking`: GGUF
    // load is a one-time, non-cancellable synchronous call, and running it
    // on tokio's shared blocking pool is exactly the hazard this stage
    // removes from the per-request inference path — a `spawn_blocking` here
    // would let a hung load linger on a shared pool slot indefinitely if
    // this future is ever raced against a timeout (below) and abandoned. A
    // dedicated thread has no such shared-resource blast radius: if the
    // timeout fires, dropping `rx` just means nobody is listening anymore,
    // and the thread still runs to completion (or hangs) on its own,
    // without holding a slot other `spawn_blocking` callers across the repo
    // depend on.
    let spawned = std::thread::Builder::new()
        .name("ene-ai-local-gguf-load".to_string())
        .spawn(move || {
            let result = LocalLlamaCppProvider::load(&cfg);
            drop(tx.send(result));
        });
    if let Err(e) = spawned {
        return Err(LlmProviderError::LocalLlm(format!(
            "failed to spawn GGUF load thread: {e}"
        )));
    }
    match tokio::time::timeout(load_timeout, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_recv_error)) => Err(LlmProviderError::LocalLlm(
            "GGUF load thread ended without a result".to_string(),
        )),
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
            if !AiConfig::is_local_provider(&proactive.provider)
                && config
                    .get_provider(&proactive.provider)
                    .is_ok_and(AiProviderDef::is_openai_compatible) =>
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
    use ene_ai::config::{AiConfig, LOCAL_PROVIDER, LocalModelDef, ProactiveAcceleration, TaskRef};

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
        cfg.local_models.insert(
            "smoke".to_string(),
            LocalModelDef {
                model_path: model,
                acceleration: accel,
                gpu_layers: "auto".to_string(),
                context_size: 1024,
                ..LocalModelDef::default()
            },
        );
        cfg.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("smoke".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
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
                parts: vec![ene_ai::message::UserMessagePart::Text {
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
            out.text.contains("should_speak"),
            "expected decision json, got: {}",
            out.text
        );
        handles.shutdown().await;
    }
}
