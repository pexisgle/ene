//! In-process llama-cpp-4 provider for proactive decisions.

mod model;
mod routing;

pub use routing::{
    DecisionProviderKind, DisabledDecisionProvider, ProactiveLlmHandles,
    build_proactive_llm_handles,
};

use async_trait::async_trait;
use ene_ai::config::ProactiveAcceleration;
use ene_ai::error::LlmProviderError;
use ene_ai::message::{LlmCompletion, LlmMessage, LlmResponseChunk};
use ene_ai::traits::LlmProvider;
use ene_ai::{
    Capability, CapabilitySet, EngineDescriptor, ResourceRegistry, StreamingLocalLlmEngine,
};
use ene_infer::{EngineConfig, EngineError, EngineHandle};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::Stream;
use tokio_util::sync::CancellationToken;

use crate::llama_cpp::{LoadSpec, resource_class_for};
use model::{LlamaChatModel, LlamaCppRequest, LlamaCppResponse};

/// Provider name / engine id, used in tracing and `EngineDescriptor::id`.
const PROVIDER_NAME: &str = "llama-cpp-local";

/// Number of decision/vision jobs allowed to queue behind the one currently
/// executing. This engine backs both the proactive decision loop and
/// (when an mmproj is loaded) vision summarization — both low-frequency,
/// one-in-flight-at-a-time workloads — so the conservative
/// `ene_ai::ConcurrencyHint::default` sizing (1 in flight, 2 queued) is
/// used as this crate's own constant rather than imported from there
/// (`ConcurrencyHint` is advisory metadata for a future multi-worker
/// orchestration layer, not something `EngineConfig::queue_depth` reads
/// directly today).
const CHAT_QUEUE_DEPTH: usize = 2;

/// No stall timeout: `mtmd.eval_chunks()` and a large prompt's `ctx.decode()`
/// are single opaque FFI calls with no natural interruption point for
/// `JobContext::tick` to run during, so any stall threshold shorter than the
/// slowest realistic call would misidentify a merely slow (but healthy) job
/// as a wedged worker and permanently disable the engine. Same reasoning as
/// `ene-voice`'s whisper/Kokoro engines and this crate's own embedding
/// engine (`crate::embedding::provider`).
const CHAT_STALL_TIMEOUT: Option<Duration> = None;

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
    /// Per-request timeout for decision completion. Maps directly onto
    /// [`ene_infer::EngineConfig::job_timeout`] — the *only* timeout in this
    /// provider's design, enforced cooperatively inside the worker via
    /// `JobContext::should_stop`. There is deliberately no second, outer
    /// `tokio::time::timeout` anywhere in this crate.
    pub request_timeout_seconds: u64,
}

/// Maps an [`ene_infer::EngineError`] to [`LlmProviderError`]. Since this
/// model's own [`ene_infer::LocalModel::Error`] *is* [`LlmProviderError`]
/// (see `model::LlamaChatModel`), `EngineError::Model` needs no further
/// wrapping/stringifying — it already carries the right type.
fn map_engine_error(err: EngineError<LlmProviderError>) -> LlmProviderError {
    match err {
        EngineError::Busy { queue_depth } => LlmProviderError::Busy { queue_depth },
        EngineError::Timeout { .. } => LlmProviderError::Timeout,
        EngineError::Cancelled => LlmProviderError::Cancelled,
        EngineError::EngineDown { reason } => {
            LlmProviderError::Provider(format!("engine down: {reason}"))
        }
        EngineError::Model(model_err) => model_err,
    }
}

/// Local decision provider backed by an in-process llama.cpp model.
///
/// Wraps a [`StreamingLocalLlmEngine`] (this crate's only consumer of
/// `ene-ai`'s blanket `LlmProvider` adapters) for ordinary chat completion —
/// real, token-by-token streaming for [`LlmProvider::create_chat_stream`],
/// since [`LlamaChatModel`] implements [`ene_infer::StreamingLocalModel`] —
/// plus a second call path — [`Self::summarize_rgb_with_cancel`] — that
/// submits directly through [`StreamingLocalLlmEngine::handle`] for raw-RGB
/// vision summarization, a request shape [`ene_ai::LlmChatRequest`] cannot
/// express. Both paths run on the *same* worker/model instance and therefore
/// naturally serialize against each other (one dedicated worker thread per
/// [`ene_infer::EngineHandle`]).
pub struct LocalLlamaCppProvider {
    engine: StreamingLocalLlmEngine<LlamaChatModel>,
}

impl LocalLlamaCppProvider {
    /// Load GGUF weights from `params` and wrap as an [`LlmProvider`].
    ///
    /// # Errors
    ///
    /// Returns [`LlmProviderError`] when the model path is invalid or the
    /// first model build fails ([`EngineHandle::try_spawn`] builds it
    /// synchronously here, so this failure is not deferred to the first
    /// [`LlmProvider::chat_completion`] call).
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
        let resource = resource_class_for(&spec)?;
        let job_timeout = Duration::from_secs(params.request_timeout_seconds.max(1));

        // `try_spawn` builds the first `LlamaChatModel` synchronously (on
        // this thread) before returning, but hands back only the
        // `EngineHandle` — `vision_detected` is a side channel the factory
        // populates while building that first model, read right after
        // `try_spawn` returns, so the descriptor's `Capability::Vision` can
        // reflect the *actual* loaded mmproj (an mmproj path can be
        // configured but silently fail to load or not report vision
        // support — see `LoadedModel::load`) without loading the GGUF a
        // second time just to inspect it.
        let vision_detected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let factory = {
            let spec = spec.clone();
            let vision_detected = Arc::clone(&vision_detected);
            move || {
                let model = LlamaChatModel::new(&spec)?;
                vision_detected.store(
                    model.supports_vision(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                Ok(model)
            }
        };
        let mut cfg = EngineConfig::new(CHAT_QUEUE_DEPTH, job_timeout);
        if let Some(stall) = CHAT_STALL_TIMEOUT {
            cfg = cfg.with_stall_timeout(stall);
        }
        let handle = EngineHandle::try_spawn(factory, cfg)?;

        let mut caps = CapabilitySet::empty()
            .with(Capability::Chat)
            .with(Capability::Grammar)
            .with(Capability::Streaming);
        if vision_detected.load(std::sync::atomic::Ordering::Relaxed) {
            caps = caps.with(Capability::Vision);
        }
        let descriptor = EngineDescriptor::new(PROVIDER_NAME, caps, resource);
        Ok(Self {
            engine: StreamingLocalLlmEngine::new(handle, descriptor),
        })
    }

    /// This engine's declared capabilities — the same
    /// [`ene_ai::CapabilitySet`] the blanket `LlmProvider` adapter already
    /// uses internally to gate vision input, so both call sites agree by
    /// construction instead of duplicating the check.
    #[must_use]
    pub fn capabilities(&self) -> CapabilitySet {
        self.engine.descriptor().capabilities
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
        self.summarize_rgb_with_cancel(width, height, rgb, system, user, CancellationToken::new())
            .await
    }

    /// Summarize an RGB8 screen capture, cooperatively cancellable via
    /// `cancel`.
    ///
    /// Cancellation flows into [`ene_infer::JobContext::should_stop`] inside
    /// the worker exactly like [`LlmProvider::chat_completion`]'s own
    /// (fresh, non-cancellable) token — this is simply the one call path in
    /// this crate that lets a caller supply its own token instead of one
    /// scoped to a single call (see `crates/ene-runtime/src/vision.rs`,
    /// which cancels an in-flight vision summary when a new user turn
    /// starts).
    pub async fn summarize_rgb_with_cancel(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        system: &str,
        user: &str,
        cancel: CancellationToken,
    ) -> Result<String, LlmProviderError> {
        if !self.capabilities().contains(Capability::Vision) {
            return Err(LlmProviderError::LocalLlm(
                "local model has no vision mmproj loaded".to_string(),
            ));
        }

        let permit = ResourceRegistry::acquire(self.engine.descriptor().resource)
            .await
            .map_err(|e| {
                LlmProviderError::LocalLlm(format!("resource admission semaphore closed: {e}"))
            })?;
        let request = LlamaCppRequest::Vision {
            system: system.to_string(),
            user: user.to_string(),
            width,
            height,
            rgb,
        };
        let outcome = self.engine.handle().submit(request, cancel).await;
        drop(permit);

        outcome
            .map(|r: LlamaCppResponse| r.text)
            .map_err(map_engine_error)
    }

    /// No-op kept for [`ProactiveLlmHandles::shutdown`] API compatibility.
    #[expect(
        clippy::unused_async,
        reason = "async kept for ProactiveLlmHandles::shutdown API"
    )]
    pub async fn shutdown(&self) {}
}

#[async_trait]
impl LlmProvider for LocalLlamaCppProvider {
    fn name(&self) -> &str {
        self.engine.descriptor().id.as_str()
    }

    async fn create_chat_stream(
        &self,
        messages: &[LlmMessage],
        tools: &[ene_plugin_proto::ToolSpec],
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<LlmResponseChunk, LlmProviderError>> + Send>>,
        LlmProviderError,
    > {
        // `StreamingLocalLlmEngine::create_chat_stream` already rejects
        // non-empty `tools` when `Capability::Tools` is not declared — this
        // engine's descriptor never declares it (the local decision provider
        // does not allow tool calls), so the rejection comes from the adapter.
        self.engine.create_chat_stream(messages, tools).await
    }

    async fn chat_completion(
        &self,
        messages: &[LlmMessage],
        json_schema: Option<serde_json::Value>,
    ) -> Result<LlmCompletion, LlmProviderError> {
        self.engine.chat_completion(messages, json_schema).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_llm::routing::build_proactive_llm_handles;
    use ene_ai::config::{AiConfig, LOCAL_PROVIDER, LocalModelDef, ProactiveAcceleration, TaskRef};

    /// A stub `openai`-kind factory standing in for the plugin-provided
    /// backend, so cloud-decision routing can be exercised without a plugin
    /// host. Registered for the duration of the cloud tests below.
    struct StubCloudFactory;

    impl ene_ai::LlmProviderFactory for StubCloudFactory {
        fn provider_name(&self) -> &str {
            "openai"
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
            _task: &TaskRef,
        ) -> Result<Box<dyn ene_ai::LlmProvider>, LlmProviderError> {
            Ok(Box::new(StubCloudProvider))
        }
    }

    /// A stub `LlmProvider` that never talks to a network.
    struct StubCloudProvider;

    #[async_trait]
    impl ene_ai::LlmProvider for StubCloudProvider {
        fn name(&self) -> &str {
            "openai-stub"
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
            Ok(LlmCompletion::text_only("stub".to_string()))
        }
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
        ene_ai::LlmProviderRegistry::register(std::sync::Arc::new(StubCloudFactory));
        let cfg = test_config();
        let handles = build_proactive_llm_handles(&cfg).await.expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
        assert_eq!(handles.generation_model, "gpt-4o-mini");
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn proactive_openai_task_drives_generation_model() {
        ene_ai::LlmProviderRegistry::register(std::sync::Arc::new(StubCloudFactory));
        let mut cfg = test_config();
        let mut ai = cfg.get_section::<AiConfig>().expect("ai config");
        ai.tasks.proactive = Some(TaskRef {
            provider: "default".to_string(),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        cfg.set_section(&ai).expect("ai config merges");
        let handles = build_proactive_llm_handles(&cfg).await.expect("build");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Cloud);
        assert_eq!(handles.generation_model, "gpt-4o");
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn local_missing_weights_fails_closed_to_disabled() {
        let mut cfg = test_config();
        let mut ai = cfg.get_section::<AiConfig>().expect("ai config");
        ai.local_models.insert(
            "missing".to_string(),
            LocalModelDef {
                model_path: "/nonexistent/ene-missing-decision.gguf".to_string(),
                ..LocalModelDef::default()
            },
        );
        ai.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("missing".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        cfg.set_section(&ai).expect("ai config merges");
        let handles = build_proactive_llm_handles(&cfg)
            .await
            .expect("missing weights fail-closed to disabled");
        assert_eq!(handles.decision_kind, DecisionProviderKind::Disabled);
        handles.shutdown().await;
    }

    #[tokio::test]
    async fn missing_model_path_fails_closed_to_disabled() {
        let mut cfg = test_config();
        let mut ai = cfg.get_section::<AiConfig>().expect("ai config");
        ai.local_models.insert(
            "empty".to_string(),
            LocalModelDef {
                ..LocalModelDef::default()
            },
        );
        ai.tasks.proactive = Some(TaskRef {
            provider: LOCAL_PROVIDER.to_string(),
            model: Some("empty".to_string()),
            max_tokens: None,
            dimensions: None,
            query_prefix: None,
            supports_vision: false,
        });
        cfg.set_section(&ai).expect("ai config merges");
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
        let Err(err) = LocalLlamaCppProvider::load(&params) else {
            panic!("expected empty path to fail");
        };
        assert!(matches!(err, LlmProviderError::LocalLlm(_)));
    }

    /// Manual smoke: load a local GGUF and run a short JSON-schema constrained turn.
    ///
    /// ```text
    /// ENE_SMOKE_GGUF=assets/models/gguf/gemma-4-E4B-it-Q4_0.gguf \
    ///   cargo test -p ene-ai-local -- --ignored smoke_gguf_load_and_grammar --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "requires a local GGUF path via ENE_SMOKE_GGUF"]
    #[expect(
        clippy::print_stderr,
        reason = "manual smoke test run with --nocapture to inspect model output"
    )]
    async fn smoke_gguf_load_and_grammar() {
        let path = std::env::var("ENE_SMOKE_GGUF").expect("ENE_SMOKE_GGUF");
        let mmproj = std::env::var("ENE_SMOKE_MMPROJ").ok();
        let provider = LocalLlamaCppProvider::load(&LocalGgufLoadParams {
            model_path: path,
            mmproj_path: mmproj,
            acceleration: ProactiveAcceleration::Cpu,
            gpu_layers: "0".to_string(),
            context_size: 2048,
            request_timeout_seconds: 120,
        })
        .expect("load GGUF");

        let messages = [LlmMessage::User {
            parts: vec![ene_ai::message::UserMessagePart::Text {
                text: "Say hi in one short word.".into(),
            }],
        }];
        let plain = provider
            .chat_completion(&messages, None)
            .await
            .expect("plain completion");
        eprintln!("smoke plain: {plain:?}");
        assert!(
            !plain.text.trim().is_empty(),
            "expected non-empty plain completion"
        );

        let schema = serde_json::json!({
            "type": "object",
            "properties": { "ok": { "type": "boolean" } },
            "required": ["ok"],
            "additionalProperties": false
        });
        let schema_messages = [LlmMessage::User {
            parts: vec![ene_ai::message::UserMessagePart::Text {
                text: "Reply with JSON only: {\"ok\": true}".into(),
            }],
        }];
        let out = provider
            .chat_completion(&schema_messages, Some(schema))
            .await
            .expect("grammar completion");
        eprintln!("smoke grammar: {out:?}");
        assert!(
            !out.text.trim().is_empty(),
            "expected non-empty grammar completion"
        );
    }

    /// Manual smoke: confirm `create_chat_stream` delivers *incremental*
    /// chunks against a real llama.cpp model, not one item wrapping the
    /// whole reply — the thing this stage's streaming adapter exists to
    /// prove. Run alongside `smoke_gguf_load_and_grammar`, same GGUF.
    ///
    /// ```text
    /// ENE_SMOKE_GGUF=assets/models/gguf/gemma-4-E4B-it-Q4_0.gguf \
    ///   cargo test -p ene-ai-local -- --ignored smoke_gguf_streaming --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "requires a local GGUF path via ENE_SMOKE_GGUF"]
    #[expect(
        clippy::print_stderr,
        reason = "manual smoke test run with --nocapture to inspect model output"
    )]
    async fn smoke_gguf_streaming() {
        use tokio_stream::StreamExt;

        let path = std::env::var("ENE_SMOKE_GGUF").expect("ENE_SMOKE_GGUF");
        let mmproj = std::env::var("ENE_SMOKE_MMPROJ").ok();
        let provider = LocalLlamaCppProvider::load(&LocalGgufLoadParams {
            model_path: path,
            mmproj_path: mmproj,
            acceleration: ProactiveAcceleration::Cpu,
            gpu_layers: "0".to_string(),
            context_size: 2048,
            request_timeout_seconds: 120,
        })
        .expect("load GGUF");

        let messages = [LlmMessage::User {
            parts: vec![ene_ai::message::UserMessagePart::Text {
                text: "Count from one to five, one number per line.".into(),
            }],
        }];
        let mut stream = provider
            .create_chat_stream(&messages, &[])
            .await
            .expect("create_chat_stream");

        let mut chunk_count = 0_usize;
        let mut full_text = String::new();
        while let Some(item) = stream.next().await {
            let chunk = item.expect("chunk ok");
            if let Some(delta) = chunk.text_delta {
                eprintln!("smoke stream chunk {chunk_count}: {delta:?}");
                full_text.push_str(&delta);
            }
            chunk_count += 1;
        }
        eprintln!("smoke stream full text: {full_text:?}");
        assert!(
            chunk_count > 1,
            "expected more than one chunk from a real streaming model, got {chunk_count} \
             (a single chunk would mean this is still the one-item fallback, not real \
             token-by-token streaming)"
        );
        assert!(
            !full_text.trim().is_empty(),
            "expected non-empty text assembled from streamed chunks"
        );
    }
}
