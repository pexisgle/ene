//! Implements [`ConfigurablePlugin`] (mmproj / acceleration config and
//! per-model profiles), [`LlmPlugin`] (streaming and non-streaming chat on
//! [`LocalLlamaCppProvider`](crate::local_llm::LocalLlamaCppProvider),
//! including message-based vision when an mmproj is configured), and
//! [`EmbedPlugin`] (GGUF embeddings on
//! [`GgufEmbeddingProvider`](crate::embedding::GgufEmbeddingProvider)). Models
//! load lazily per profile key and stay resident for the process lifetime.

use async_trait::async_trait;
use ene_ai::EmbeddingKind;
use ene_ai::traits::{EmbeddingProvider, LlmProvider};
use ene_plugin::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::config;
use crate::convert;
use crate::models;

/// Budget for a non-streaming completion.
///
/// Decisions are short structured-output calls. The host's own decision
/// timeout (mind's `decision_timeout_seconds`, 15 s by default) fires before
/// this budget and abandons the request without cancelling it, so a longer
/// plugin-side budget would let a hung generation occupy the single local
/// engine for minutes (silencing proactive with `Busy`). Dropping the engine
/// call on timeout cancels the job cooperatively via `caller_gone`.
const DECISION_COMPLETION_TIMEOUT_SECS: u64 = 20;
const GENERATE_COMPLETION_TIMEOUT_SECS: u64 = 120;

/// The static capability data (`llm_spec()` / `LLM_PROVIDER_KIND` /
/// `provides()`) is generated from the `#[provider(...)]` attribute; the
/// async handlers below load models lazily per profile key.
#[derive(LlmPlugin)]
#[provider(
    kind = "local",
    streaming,
    vision,
    // A local model runs one job at a time on a dedicated worker thread; the
    // host enforces the same bound with admission control.
    concurrency = 1,
    queue_depth = 2,
    provides = "llm/chat@1, embed@1, gguf-runner@1"
)]
pub(crate) struct LocalLlmPlugin;

impl ene_plugin::ConfigurablePlugin for LocalLlmPlugin {
    fn set_config(&self, config: &Value) {
        config::set_config(config);
    }

    fn set_profiles(&self, profiles: &Value) {
        config::set_profiles(profiles);
    }

    /// Profile shape (`url` / `quantization` /
    /// `model_path` / `gpu_layers` / optional `context_size` per model) is
    /// delivered via `set_profiles` and documented in `docs/configuration.md`;
    /// the host treats profiles as opaque.
    fn config_schema(&self) -> Option<Value> {
        Some(serde_json::json!({
            "type": "object",
            "x-ene-profiles-schema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "GGUF download URL" },
                    "artifact_id": { "type": "string", "description": "Catalog artifact id for catalog-managed weights (host injects the verified CAS path as model_path)" },
                    "artifact_version": { "type": "string", "description": "Optional catalog version pin, e.g. =1.2.0 (empty = catalog default)" },
                    "model_path": { "type": "string", "description": "Local GGUF path (skips download when non-empty)" },
                    "quantization": { "type": "string", "description": "Quantization tag, e.g. q4_k_m" },
                    "gpu_layers": { "type": "integer", "minimum": 0, "description": "GPU offload layers (0 = CPU only)" },
                    "context_size": { "type": "integer", "minimum": 1, "description": "Context window in tokens" }
                }
            },
            "properties": {
                "mmproj_url": {
                    "type": "string",
                    "description": "HTTPS URL for the multimodal projector (mmproj) GGUF",
                    "x-ene-ui": { "group": "multimodal", "order": 1, "impact": "plugin_restart" }
                },
                "mmproj_path": {
                    "type": "string",
                    "description": "Optional filesystem path for mmproj (skips download when non-empty)",
                    "x-ene-ui": { "group": "multimodal", "order": 2, "impact": "plugin_restart" }
                },
                "acceleration": {
                    "type": "string",
                    "enum": ["auto", "cpu", "vulkan", "cuda"],
                    "description": "Preferred acceleration backend for llama.cpp",
                    "x-ene-ui": { "order": 0, "impact": "plugin_restart" }
                }
            }
        }))
    }
}

#[async_trait]
impl LlmPlugin for LocalLlmPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        // The class is derived from the acceleration config so the host can
        // gate this provider against other GPU users; an unreadable config
        // falls back to Cpu (requests will fail with a typed error anyway).
        let mut spec = Self::llm_spec();
        match config::resource_class() {
            Ok(class) => spec.resource_class = class,
            Err(e) => {
                tracing::warn!(
                    component = "LocalLlmPlugin",
                    error = %e,
                    "declaring Cpu resource class: acceleration config unreadable"
                );
            }
        }
        vec![spec]
    }

    async fn create_chat_stream(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        // The local core's generation cap is hardcoded at 320/256 tokens;
        // honoring the wire `max_tokens` is a later-slice item.
        _max_tokens: Option<u32>,
        messages: Vec<Value>,
        tools: Vec<Value>,
    ) -> Result<PluginStream, PluginError> {
        ensure_kind(kind)?;
        if !tools.is_empty() {
            return Err(PluginError::not_supported(
                "local models do not support tool calls",
            ));
        }
        let messages = convert::to_llm_messages(&messages)?;
        let provider = models::chat_provider(&model).await?;
        let stream = provider
            .create_chat_stream(&messages, &[])
            .await
            .map_err(|e| convert::map_llm_error(&e))?;

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            // The engine handle lives in the provider; keep it alive for as
            // long as the stream runs even if the registry entry is dropped.
            let _keep_alive = provider;
            let mut stream = stream;
            while let Some(item) = stream.next().await {
                let mapped = item
                    .map_err(|e| convert::map_llm_error(&e))
                    .map(convert::map_stream_chunk);
                if tx.send(mapped).await.is_err() {
                    break;
                }
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn chat_completion(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        _max_tokens: Option<u32>,
        messages: Vec<Value>,
        json_schema: Option<Value>,
    ) -> Result<PluginCompletion, PluginError> {
        self.complete(
            kind,
            model,
            messages,
            json_schema,
            DECISION_COMPLETION_TIMEOUT_SECS,
        )
        .await
    }
}

impl LocalLlmPlugin {
    async fn complete(
        &self,
        kind: &str,
        model: String,
        messages: Vec<Value>,
        json_schema: Option<Value>,
        budget_secs: u64,
    ) -> Result<PluginCompletion, PluginError> {
        ensure_kind(kind)?;
        let messages = convert::to_llm_messages(&messages)?;
        let provider = models::chat_provider(&model).await?;
        let completion = tokio::time::timeout(
            Duration::from_secs(budget_secs),
            provider.chat_completion(&messages, json_schema),
        )
        .await
        .map_err(|_| {
            PluginError::provider(format!(
                "local completion exceeded the {budget_secs}s completion budget"
            ))
        })?
        .map_err(|e| convert::map_llm_error(&e))?;
        Ok(PluginCompletion {
            text: completion.text,
            usage: completion.usage,
        })
    }
}

#[async_trait]
impl EmbedPlugin for LocalLlmPlugin {
    fn embed_providers(&self) -> Vec<String> {
        vec![Self::LLM_PROVIDER_KIND.to_string()]
    }

    async fn embed_batch(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        dimensions: Option<u32>,
        items: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        ensure_kind(kind)?;
        if items.is_empty() {
            return Ok(Vec::new());
        }
        for text in &items {
            if text.trim().is_empty() {
                return Err(PluginError::provider(
                    "cannot embed empty text; refusing to pollute the vector store",
                ));
            }
        }
        let provider = models::embed_provider(&model).await?;
        if let Some(requested) = dimensions
            && requested > 0
            && usize::try_from(requested).is_ok_and(|dims| dims != provider.dimensions())
        {
            return Err(PluginError::provider(format!(
                "model {model:?} produces {} dims but {requested} were requested",
                provider.dimensions()
            )));
        }
        // Embedding kinds are dropped at the IPC boundary, so every item is
        // embedded with the document prefix; the host's configured query
        // prefix remains the query-side knob (see the Slice C plan).
        let batch: Vec<(&str, EmbeddingKind)> = items
            .iter()
            .map(|text| (text.as_str(), EmbeddingKind::Summary))
            .collect();
        let vectors = provider
            .embed_batch(&batch)
            .await
            .map_err(|e| convert::map_embed_error(&e))?;
        Ok(vectors)
    }
}

/// `gguf-runner@1` `generate` request: prompt plus optional JSON schema.
#[derive(Deserialize)]
struct GenerateRequest {
    model: String,
    prompt: String,
    #[serde(default)]
    json_schema: Option<Value>,
}

/// `gguf-runner@1` `embed` request.
#[derive(Deserialize)]
struct EmbedRequest {
    model: String,
    texts: Vec<String>,
}

/// `gguf-runner@1` `unload` request.
#[derive(Deserialize)]
struct UnloadRequest {
    model: String,
}

#[async_trait]
impl CapabilityProvider for LocalLlmPlugin {
    /// Serves the published `gguf-runner@1` method contract by delegating to
    /// the plugin's own chat / embedding paths, so mediated calls share the
    /// same model registry, completion budget, and error mapping as
    /// host-driven requests.
    async fn call_capability(
        &self,
        capability: &CapabilityRef,
        method: &str,
        payload: Value,
    ) -> Result<Value, PluginError> {
        if capability.as_str() != "gguf-runner@1" {
            return Err(PluginError::not_supported(format!(
                "capability {capability}"
            )));
        }
        match method {
            "generate" => {
                let request: GenerateRequest = serde_json::from_value(payload)
                    .map_err(|e| PluginError::provider(format!("invalid generate request: {e}")))?;
                let messages = vec![serde_json::json!({
                    "role": "user",
                    "parts": [{ "Text": { "text": request.prompt } }]
                })];
                let completion = self
                    .complete(
                        Self::LLM_PROVIDER_KIND,
                        request.model,
                        messages,
                        request.json_schema,
                        GENERATE_COMPLETION_TIMEOUT_SECS,
                    )
                    .await?;
                Ok(serde_json::json!({ "text": completion.text }))
            }
            "embed" => {
                let request: EmbedRequest = serde_json::from_value(payload)
                    .map_err(|e| PluginError::provider(format!("invalid embed request: {e}")))?;
                let vectors = EmbedPlugin::embed_batch(
                    self,
                    Self::LLM_PROVIDER_KIND,
                    serde_json::json!({}),
                    request.model,
                    None,
                    request.texts,
                )
                .await?;
                Ok(serde_json::json!({ "embeddings": vectors }))
            }
            "unload" => {
                let request: UnloadRequest = serde_json::from_value(payload)
                    .map_err(|e| PluginError::provider(format!("invalid unload request: {e}")))?;
                models::unload(&request.model);
                Ok(serde_json::json!({ "ok": true }))
            }
            _ => Err(PluginError::not_supported(format!("method {method}"))),
        }
    }
}

fn ensure_kind(kind: &str) -> Result<(), PluginError> {
    if kind == LocalLlmPlugin::LLM_PROVIDER_KIND {
        Ok(())
    } else {
        Err(PluginError::not_supported(format!("provider kind: {kind}")))
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "contract tests use expect/panic for assertions"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use ene_plugin::{ConfigurablePlugin, LlmPlugin, PluginDispatch};
    use ene_plugin_proto::{
        CapabilityRef, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginIpcRequest, PluginIpcResponse,
        VersionRange, WireFormat, cleanup_path, read_plugin_response, write_plugin_request,
    };

    /// Counter for unique socket paths across parallel test runs.
    static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn test_socket_path(name: &str) -> PathBuf {
        let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        PathBuf::from(format!(
            "/tmp/ene-local-llm-test-{}-{id}-{name}.sock",
            std::process::id()
        ))
    }

    fn dispatch() -> PluginDispatch {
        PluginDispatch::new(
            None,
            Some(std::sync::Arc::new(super::LocalLlmPlugin)),
            Some(std::sync::Arc::new(super::LocalLlmPlugin)),
            None,
            None,
        )
        .with_capability_declarations(super::LocalLlmPlugin::provides(), Vec::new())
    }

    /// The full host round-trip: handshake (with config + profiles) →
    /// capability declarations → live `SetConfig` → `GetConfigSchema` →
    /// inference error for an unconfigured model.
    #[tokio::test]
    async fn handshake_declares_capabilities_and_round_trips_config() {
        let socket_path = test_socket_path("contract");
        cleanup_path(&socket_path);
        // SAFETY: test-only env mutation; this is the only test touching the
        // socket env var in this binary.
        unsafe {
            std::env::set_var("ENE_PLUGIN_SOCKET", &socket_path);
        }
        let server = tokio::spawn(async move {
            drop(ene_plugin::run_plugin_server(dispatch()).await);
        });
        // The server task binds asynchronously; retry the connect briefly so
        // a slow CI runner does not flake on the first attempt.
        let mut stream = None;
        for _ in 0..10 {
            if let Ok(connected) = IpcStream::connect(&socket_path).await {
                stream = Some(connected);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let mut stream = stream.expect("connect after retries");
        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange {
                    min: PLUGIN_IPC_PROTOCOL_VERSION,
                    max: PLUGIN_IPC_PROTOCOL_VERSION,
                },
                sandbox: ene_plugin_proto::SandboxConfigData::default(),
                plugin_config: Some(serde_json::json!({
                    "mmproj_url": "https://cdn.example/mmproj.gguf",
                    "acceleration": "vulkan"
                })),
                plugin_profiles: Some(serde_json::json!({
                    "gemma-4-e4b": {
                        "url": "https://cdn.example/gemma-4-e4b.gguf",
                        "quantization": "Q4_0",
                        "model_path": "",
                        "gpu_layers": "auto"
                    }
                })),
            },
            WireFormat::Json,
        )
        .await
        .expect("write handshake");

        let ack = read_plugin_response(&mut stream, WireFormat::Json)
            .await
            .expect("read handshake ack")
            .expect("handshake ack frame");
        let PluginIpcResponse::HandshakeAck {
            version,
            capabilities,
        } = ack
        else {
            panic!("expected HandshakeAck, got {ack:?}");
        };
        assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);

        assert_eq!(capabilities.llm_providers.len(), 1);
        let spec = &capabilities.llm_providers[0];
        assert_eq!(spec.kind, "local");
        assert!(spec.supports_streaming);
        assert!(spec.supports_vision);
        assert_eq!(spec.concurrency.max_in_flight, 1);
        assert_eq!(spec.concurrency.queue_depth, 2);
        assert_eq!(capabilities.embed_providers, vec!["local".to_string()]);
        assert_eq!(
            capabilities.provides,
            vec![
                CapabilityRef::parse("llm/chat@1").expect("static capability"),
                CapabilityRef::parse("embed@1").expect("static capability"),
                CapabilityRef::parse("gguf-runner@1").expect("static capability"),
            ]
        );
        assert!(capabilities.requires.is_empty());

        // v6 negotiated: subsequent frames are MessagePack.
        let format = WireFormat::MsgPack;
        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::SetConfig {
                request_id: "req-set".to_string(),
                config: serde_json::json!({"acceleration": "cuda"}),
                profiles: Some(serde_json::json!({})),
            },
            format,
        )
        .await
        .expect("write set_config");
        let resp = read_plugin_response(&mut stream, format)
            .await
            .expect("read set_config response")
            .expect("set_config frame");
        assert!(matches!(resp, PluginIpcResponse::ConfigApplied { .. }));

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::GetConfigSchema {
                request_id: "req-schema".to_string(),
            },
            format,
        )
        .await
        .expect("write get_config_schema");
        let resp = read_plugin_response(&mut stream, format)
            .await
            .expect("read schema response")
            .expect("schema frame");
        let PluginIpcResponse::ConfigSchema { schema, .. } = resp else {
            panic!("expected ConfigSchema, got {resp:?}");
        };
        let schema = schema.expect("schema present");
        assert_eq!(
            schema.pointer("/properties/acceleration/enum"),
            Some(&serde_json::json!(["auto", "cpu", "vulkan", "cuda"]))
        );

        // Inference for an unconfigured model fails typed (no model load, no
        // panic) — exercises the real profile-lookup path.
        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::CreateChatStream {
                request_id: "req-chat".to_string(),
                provider_kind: "local".to_string(),
                provider_config: serde_json::json!({}),
                model: "no-such-model".to_string(),
                max_tokens: None,
                messages: Vec::new(),
                tools: Vec::new(),
            },
            format,
        )
        .await
        .expect("write create_chat_stream");
        let resp = read_plugin_response(&mut stream, format)
            .await
            .expect("read stream error")
            .expect("stream error frame");
        let PluginIpcResponse::StreamError { message, .. } = resp else {
            panic!("expected StreamError for unconfigured model, got {resp:?}");
        };
        assert!(
            message.contains("profile"),
            "unconfigured model must fail with a profile error: {message}"
        );

        server.abort();
        cleanup_path(&socket_path);
    }

    #[test]
    fn config_schema_covers_llama_cpp_settings() {
        let schema = super::LocalLlmPlugin.config_schema().expect("schema");
        for key in ["mmproj_url", "mmproj_path", "acceleration"] {
            assert!(
                schema.pointer(&format!("/properties/{key}")).is_some(),
                "schema must cover {key}"
            );
        }
    }

    #[test]
    fn capabilities_advertise_local_kind_and_provides() {
        use ene_plugin::ResourceClass;

        assert_eq!(super::LocalLlmPlugin::LLM_PROVIDER_KIND, "local");
        assert_eq!(
            super::LocalLlmPlugin::provides(),
            vec![
                CapabilityRef::parse("llm/chat@1").expect("static capability"),
                CapabilityRef::parse("embed@1").expect("static capability"),
                CapabilityRef::parse("gguf-runner@1").expect("static capability"),
            ]
        );

        super::config::set_config(&serde_json::json!({"acceleration": "vulkan"}));
        let spec = super::LocalLlmPlugin.llm_capabilities();
        assert_eq!(
            spec.first().expect("one spec").resource_class,
            ResourceClass::Gpu { device: 0 }
        );
        super::config::set_config(&serde_json::json!({"acceleration": "cpu"}));
        let spec = super::LocalLlmPlugin.llm_capabilities();
        assert_eq!(
            spec.first().expect("one spec").resource_class,
            ResourceClass::Cpu
        );
        super::config::set_config(&serde_json::json!({}));
    }
}
