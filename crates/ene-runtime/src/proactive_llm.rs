//! Proactive decision-provider routing.
//!
//! The decision classifier for proactive speech runs either on a cloud
//! OpenAI-compatible provider (in-process routing through the plugin
//! registry) or on the local GGUF provider plugin (`kind = "local"`, the
//! `ene-plugin-llama-cpp` binary). Local routing is a registry lookup plus a
//! warm-up call: the plugin loads the model lazily on its first request, so
//! the warm-up (run in the actor's background init task) restores the old
//! in-process semantics where decision ticks and vision requests were gated
//! on the model being loaded rather than burning their own timeouts on the
//! load.

use async_trait::async_trait;
use ene_ai::config::AiConfig;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmCompletion, LlmMessage, LlmResponseChunk};
use ene_ai::traits::{LlmProvider, LlmProviderRegistry};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;

const DECISION_MAX_TOKENS: u32 = 256;

/// Budget for the local decision warm-up (model load + a short completion).
///
/// Matches the old in-process GGUF load budget, so a model that loaded under
/// the old path still warms up under the plugin path.
const LOCAL_WARMUP_TIMEOUT: Duration = Duration::from_mins(5);

/// Which decision backend is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionProviderKind {
    /// The `local` plugin provider (llama.cpp over IPC).
    Local,
    /// Cloud OpenAI-compatible.
    Cloud,
    /// Always silent (local backend unavailable; fail-closed).
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
    /// Local provider (if any) for screen-image vision.
    local: Option<Arc<dyn LlmProvider>>,
}

impl ProactiveLlmHandles {
    /// Local provider when the decision backend is plugin-routed local.
    #[must_use]
    pub fn local(&self) -> Option<&Arc<dyn LlmProvider>> {
        self.local.as_ref()
    }
}

/// Build decision + generation routing from the full configuration.
///
/// When `tasks.proactive` requests `provider: "local"`, an unavailable
/// backend fails closed to [`DisabledDecisionProvider`] — never silently
/// route observation context to a cloud decision provider. Transport-level
/// warm-up failures (plugin restarting) are returned as `Err` so the actor
/// retries on a later tick.
pub async fn build_proactive_llm_handles(
    config: &ene_config::EneConfig,
) -> Result<ProactiveLlmHandles, LlmProviderError> {
    let ai_config = config
        .get_section::<AiConfig>()
        .map_err(|e| LlmProviderError::Provider(format!("Failed to parse AI config: {e}")))?;
    // Validates the generation side (the chat task must resolve even when
    // the decision backend is local) before any decision routing happens.
    ai_config.resolve_proactive_generation()?;
    if let Some(proactive) = ai_config.tasks.proactive.as_ref()
        && AiConfig::is_local_provider(&proactive.provider)
    {
        if let Err(e) = ai_config.resolve_local_model_for_task(proactive) {
            tracing::warn!(
                component = "Proactive",
                error = %e,
                "Local decision config invalid; falling back to disabled (fail-closed)"
            );
            return Ok(disabled_handles());
        }
        let provider: Arc<dyn LlmProvider> = match LlmProviderRegistry::create_provider(
            ene_ai::LOCAL_PROVIDER,
            config,
            proactive,
        ) {
            Ok(provider) => Arc::from(provider),
            Err(e) => {
                tracing::warn!(
                    component = "Proactive",
                    error = %e,
                    "Local decision provider unavailable; falling back to disabled (fail-closed)"
                );
                return Ok(disabled_handles());
            }
        };
        match warm_up_local_provider(provider.as_ref()).await {
            Ok(()) => {}
            Err(e) if e.is_retryable() => {
                tracing::warn!(
                    component = "Proactive",
                    error = %e,
                    "Local decision warm-up interrupted; will retry"
                );
                return Err(e);
            }
            Err(e) => {
                tracing::warn!(
                    component = "Proactive",
                    error = %e,
                    "Local decision model failed to load; falling back to disabled (fail-closed)"
                );
                return Ok(disabled_handles());
            }
        }
        return Ok(ProactiveLlmHandles {
            decision: Arc::clone(&provider),
            decision_kind: DecisionProviderKind::Local,
            local: Some(provider),
        });
    }

    let cloud = build_cloud_decision_provider(config)?;
    Ok(ProactiveLlmHandles {
        decision: cloud,
        decision_kind: DecisionProviderKind::Cloud,
        local: None,
    })
}

fn disabled_handles() -> ProactiveLlmHandles {
    ProactiveLlmHandles {
        decision: Arc::new(DisabledDecisionProvider),
        decision_kind: DecisionProviderKind::Disabled,
        local: None,
    }
}

/// Issues one trivial completion so the plugin loads the local model
/// (lazily, on first request) inside the background init task.
async fn warm_up_local_provider(provider: &dyn LlmProvider) -> Result<(), LlmProviderError> {
    let messages = [LlmMessage::System {
        content: "ping".to_string(),
    }];
    match tokio::time::timeout(
        LOCAL_WARMUP_TIMEOUT,
        provider.chat_completion(&messages, None),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(LlmProviderError::Timeout),
    }
}

fn build_cloud_decision_provider(
    config: &ene_config::EneConfig,
) -> Result<Arc<dyn LlmProvider>, LlmProviderError> {
    let ai_config = config.get_section::<AiConfig>().unwrap_or_default();
    let task = match ai_config.tasks.proactive.as_ref() {
        Some(proactive)
            if !AiConfig::is_local_provider(&proactive.provider)
                && ai_config
                    .get_provider(&proactive.provider)
                    .is_ok_and(ene_ai::AiProviderDef::is_openai_compatible) =>
        {
            proactive
        }
        _ => &ai_config.tasks.chat,
    };
    // The decision classifier is a short structured-output call: cap the
    // completion at `DECISION_MAX_TOKENS` via the task override, which the
    // plugin-backed provider honors, and disable thinking so reasoning models
    // (o3-class, deepseek-reasoner, gpt-5, not just MiMo) answer in `content`
    // instead of stalling on `reasoning_content`.
    let mut decision_task = task.clone();
    decision_task.max_tokens = Some(DECISION_MAX_TOKENS);
    decision_task.thinking_disabled = true;
    let provider = ene_ai::create_chat_provider_for_task(config, &decision_task)?;
    Ok(Arc::from(provider))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use expect/panic for assertions"
)]
mod tests {
    use super::*;
    use ene_ai::LlmProviderFactory;
    use ene_ai::config::{LOCAL_PROVIDER, LocalModelDef, TaskRef};

    /// Serializes tests that read or write the process-global
    /// [`LlmProviderRegistry`]: several cases register a stub under the fixed
    /// `"local"` kind, so parallel execution would make a registry-miss test
    /// observe another test's stub (and vice versa).
    static REGISTRY_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// How a stub provider's `chat_completion` should fail.
    #[derive(Clone, Copy)]
    enum StubFailure {
        Transport,
        Load,
    }

    /// Stub factory standing in for the plugin host's registry entries.
    struct StubFactory {
        kind: &'static str,
        failure: Option<StubFailure>,
    }

    impl LlmProviderFactory for StubFactory {
        fn provider_name(&self) -> &'static str {
            self.kind
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
            _task: &TaskRef,
        ) -> Result<Box<dyn LlmProvider>, LlmProviderError> {
            Ok(Box::new(StubProvider {
                failure: self.failure,
            }))
        }
    }

    /// A stub `LlmProvider` that never talks to a network.
    struct StubProvider {
        failure: Option<StubFailure>,
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
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
                "stub provider does not stream".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<LlmCompletion, LlmProviderError> {
            match &self.failure {
                Some(StubFailure::Transport) => {
                    Err(LlmProviderError::Network("stub transport down".to_string()))
                }
                Some(StubFailure::Load) => Err(LlmProviderError::Provider(
                    "stub model load failed".to_string(),
                )),
                None => Ok(LlmCompletion::text_only("stub".to_string())),
            }
        }
    }

    /// Deregisters the stub factory on drop so the global registry is not
    /// polluted across tests.
    struct FactoryGuard {
        kind: &'static str,
        factory: Arc<dyn LlmProviderFactory>,
    }

    impl Drop for FactoryGuard {
        fn drop(&mut self) {
            LlmProviderRegistry::deregister_if_matches(self.kind, &self.factory);
        }
    }

    fn register_stub(kind: &'static str, failure: Option<StubFailure>) -> FactoryGuard {
        let factory: Arc<dyn LlmProviderFactory> = Arc::new(StubFactory { kind, failure });
        LlmProviderRegistry::register(Arc::clone(&factory));
        FactoryGuard { kind, factory }
    }

    fn test_config() -> ene_config::EneConfig {
        let mut ai = AiConfig::default();
        if let Some(def) = ai.providers.get_mut("default") {
            def.base_url = "https://api.openai.com/v1".to_string();
        }
        let mut config = ene_config::EneConfig::default();
        config.set_section(&ai).expect("ai config merges");
        config
    }

    #[tokio::test]
    async fn cloud_provider_returns_cloud_decision() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let _guard = register_stub("openai", None);
        let handles = build_proactive_llm_handles(&test_config())
            .await
            .expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
        assert!(handles.local().is_none());
    }

    #[tokio::test]
    async fn proactive_openai_task_drives_generation_model() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let _guard = register_stub("openai", None);
        let mut cfg = test_config();
        let mut ai = cfg.get_section::<AiConfig>().expect("ai config");
        ai.tasks.proactive = Some(TaskRef {
            provider: "default".to_string(),
            model: Some("gpt-4o".to_string()),
            ..TaskRef::default()
        });
        cfg.set_section(&ai).expect("ai config merges");
        let handles = build_proactive_llm_handles(&cfg).await.expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
    }

    fn local_proactive_config(model: Option<&str>) -> ene_config::EneConfig {
        let mut cfg = test_config();
        let mut ai = cfg.get_section::<AiConfig>().expect("ai config");
        if let Some(name) = model {
            ai.local_models.insert(
                name.to_string(),
                LocalModelDef {
                    model_path: "/nonexistent/ene-missing-decision.gguf".to_string(),
                    ..LocalModelDef::default()
                },
            );
        }
        ai.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: model.map(str::to_string),
            ..TaskRef::default()
        });
        cfg.set_section(&ai).expect("ai config merges");
        cfg
    }

    #[tokio::test]
    async fn local_task_without_model_entry_fails_closed_to_disabled() {
        let handles = build_proactive_llm_handles(&local_proactive_config(None))
            .await
            .expect("missing model entry fails closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
    }

    #[tokio::test]
    async fn local_task_without_registered_factory_fails_closed_to_disabled() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let handles = build_proactive_llm_handles(&local_proactive_config(Some("missing")))
            .await
            .expect("missing weights fail closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
    }

    #[tokio::test]
    async fn local_warm_up_success_returns_local_handles() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let _guard = register_stub(LOCAL_PROVIDER, None);
        let handles = build_proactive_llm_handles(&local_proactive_config(Some("missing")))
            .await
            .expect("warm-up success");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Local);
        assert!(handles.local().is_some());
    }

    #[tokio::test]
    async fn local_warm_up_transport_failure_is_retryable() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let _guard = register_stub(LOCAL_PROVIDER, Some(StubFailure::Transport));
        let Err(err) = build_proactive_llm_handles(&local_proactive_config(Some("missing"))).await
        else {
            panic!("transport failure must surface as Err for retry")
        };
        assert!(err.is_retryable(), "err: {err}");
    }

    #[tokio::test]
    async fn local_warm_up_load_failure_fails_closed_to_disabled() {
        let _registry_guard = REGISTRY_TESTS.lock().await;
        let _guard = register_stub(LOCAL_PROVIDER, Some(StubFailure::Load));
        let handles = build_proactive_llm_handles(&local_proactive_config(Some("missing")))
            .await
            .expect("load failure fails closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
    }
}
