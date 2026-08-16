//! Local GGUF provider plugin: chat streaming, completion, and embeddings.
//!
//! Implements [`ConfigurablePlugin`] (sidecar path / mmproj / acceleration
//! config and per-model profiles), [`LlmPlugin`] (streaming and non-streaming
//! chat through a managed `llama-server` sidecar, including message-based
//! vision when an mmproj is configured), and [`EmbedPlugin`] (GGUF embeddings
//! via the sidecar's OpenAI-compatible route). Models are served by the
//! sidecar's router mode and load lazily per profile key on first request.

use std::time::Duration;

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde::Deserialize;
use serde_json::Value;

use crate::config;
use crate::convert;
use crate::models;

/// Budget for a non-streaming completion.
///
/// Decisions are short structured-output calls. The host's own decision
/// timeout (mind's `decision_timeout_seconds`, 15 s by default) fires before
/// this budget and abandons the request without cancelling it, so a longer
/// plugin-side budget would let a hung generation occupy the sidecar's slot
/// for minutes. Dropping the HTTP call on timeout aborts the sidecar-side
/// generation.
const DECISION_COMPLETION_TIMEOUT_SECS: u64 = 20;

/// Local GGUF inference provider plugin (llama-server sidecar).
///
/// The static capability data (`llm_spec()` / `LLM_PROVIDER_KIND` /
/// `provides()`) is generated from the `#[provider(...)]` attribute; the
/// async handlers below resolve profiles and ensure the sidecar lazily.
#[derive(LlmPlugin)]
#[provider(
    kind = "local",
    streaming,
    vision,
    // The sidecar runs one job at a time on its own slots; the host enforces
    // the same bound with admission control.
    concurrency = 1,
    queue_depth = 2,
    provides = "llm/chat@1, embed@1, gguf-runner@1"
)]
pub(crate) struct LlamaServerPlugin;

impl ene_plugin::ConfigurablePlugin for LlamaServerPlugin {
    fn set_config(&self, config: &Value) {
        config::set_config(config);
    }

    fn set_profiles(&self, profiles: &Value) {
        config::set_profiles(profiles);
    }

    /// Advertises the config schema. Profile shape (`url` / `quantization` /
    /// `model_path` / `gpu_layers` / optional `context_size` / `dimensions`
    /// per model) is delivered via `set_profiles` and documented in
    /// `docs/configuration.md`; the host treats profiles as opaque.
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
                    "context_size": { "type": "integer", "minimum": 1, "description": "Context window in tokens" },
                    "dimensions": { "type": "integer", "minimum": 1, "description": "Embedding dimensions for embedding-capable models" }
                }
            },
            "properties": {
                "server_path": {
                    "type": "string",
                    "description": "Path to the llama-server executable (default: beside the plugin binary, then PATH)",
                    "x-ene-ui": { "group": "server", "order": 0, "impact": "plugin_restart" }
                },
                "server_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra command-line arguments passed to llama-server",
                    "x-ene-ui": { "group": "server", "order": 1, "impact": "plugin_restart" }
                },
                "startup_timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How long to wait for llama-server /health after spawning",
                    "x-ene-ui": { "group": "server", "order": 2, "impact": "plugin_restart" }
                },
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
                    "description": "Preferred acceleration backend for llama-server",
                    "x-ene-ui": { "order": 3, "impact": "plugin_restart" }
                }
            }
        }))
    }
}

#[async_trait]
impl LlmPlugin for LlamaServerPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        // The class is derived from the acceleration config so the host can
        // gate this provider against other GPU users; an unreadable config
        // falls back to Cpu (requests will fail with a typed error anyway).
        let mut spec = Self::llm_spec();
        match config::resource_class() {
            Ok(class) => spec.resource_class = class,
            Err(e) => {
                tracing::warn!(
                    component = "LlamaServerPlugin",
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
        max_tokens: Option<u32>,
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
        let oai_messages = convert::messages_to_oai(&messages)?;
        let client = models::chat_provider(&model).await?;
        client
            .chat_stream(&model, oai_messages, max_tokens, None)
            .await
    }

    async fn chat_completion(
        &self,
        kind: &str,
        _config: Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<Value>,
        json_schema: Option<Value>,
    ) -> Result<PluginCompletion, PluginError> {
        ensure_kind(kind)?;
        let messages = convert::to_llm_messages(&messages)?;
        let oai_messages = convert::messages_to_oai(&messages)?;
        let client = models::chat_provider(&model).await?;
        tokio::time::timeout(
            Duration::from_secs(DECISION_COMPLETION_TIMEOUT_SECS),
            client.chat_completion(&model, oai_messages, max_tokens, json_schema),
        )
        .await
        .map_err(|_| {
            PluginError::provider(format!(
                "local completion exceeded the {DECISION_COMPLETION_TIMEOUT_SECS}s decision budget"
            ))
        })?
    }
}

#[async_trait]
impl EmbedPlugin for LlamaServerPlugin {
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
        let (client, dims) = models::embed_provider(&model).await?;
        if let Some(requested) = dimensions
            && requested > 0
            && usize::try_from(requested).is_ok_and(|measured| measured != dims)
        {
            return Err(PluginError::provider(format!(
                "model {model:?} produces {dims} dims but {requested} were requested",
            )));
        }
        // Embedding kinds are dropped at the IPC boundary, so every item is
        // embedded with the document prefix; the host's configured query
        // prefix remains the query-side knob (same contract as the
        // in-process plugin).
        let prefixed: Vec<String> = items
            .iter()
            .map(|text| format!("Document: {text}"))
            .collect();
        client.embed_batch(&model, &prefixed).await
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
impl CapabilityProvider for LlamaServerPlugin {
    /// Serves the published `gguf-runner@1` method contract by delegating to
    /// the plugin's own chat / embedding paths, so mediated calls share the
    /// same profile registry, completion budget, and error mapping as
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
                let completion = LlmPlugin::chat_completion(
                    self,
                    Self::LLM_PROVIDER_KIND,
                    serde_json::json!({}),
                    request.model,
                    None,
                    messages,
                    request.json_schema,
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
                models::unload(&request.model).await;
                Ok(serde_json::json!({ "ok": true }))
            }
            _ => Err(PluginError::not_supported(format!("method {method}"))),
        }
    }
}

fn ensure_kind(kind: &str) -> Result<(), PluginError> {
    if kind == LlamaServerPlugin::LLM_PROVIDER_KIND {
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
            "/tmp/ene-llama-server-test-{}-{id}-{name}.sock",
            std::process::id()
        ))
    }

    fn dispatch() -> PluginDispatch {
        PluginDispatch::new(
            None,
            Some(std::sync::Arc::new(super::LlamaServerPlugin)),
            Some(std::sync::Arc::new(super::LlamaServerPlugin)),
            None,
            None,
        )
        .with_capability_declarations(super::LlamaServerPlugin::provides(), Vec::new())
    }

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
                    "server_path": "/opt/llama/llama-server",
                    "startup_timeout_secs": 5,
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

        // v6+ negotiated: subsequent frames are MessagePack.
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
        assert!(schema.pointer("/properties/server_path").is_some());
        assert!(schema.pointer("/properties/server_args").is_some());
        assert!(schema.pointer("/properties/startup_timeout_secs").is_some());

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
    fn config_schema_covers_sidecar_settings() {
        let schema = super::LlamaServerPlugin.config_schema().expect("schema");
        for key in [
            "server_path",
            "server_args",
            "startup_timeout_secs",
            "mmproj_url",
            "mmproj_path",
            "acceleration",
        ] {
            assert!(
                schema.pointer(&format!("/properties/{key}")).is_some(),
                "schema must cover {key}"
            );
        }
    }

    #[test]
    fn capabilities_advertise_local_kind_and_provides() {
        use ene_plugin::ResourceClass;

        assert_eq!(super::LlamaServerPlugin::LLM_PROVIDER_KIND, "local");
        assert_eq!(
            super::LlamaServerPlugin::provides(),
            vec![
                CapabilityRef::parse("llm/chat@1").expect("static capability"),
                CapabilityRef::parse("embed@1").expect("static capability"),
                CapabilityRef::parse("gguf-runner@1").expect("static capability"),
            ]
        );

        super::config::set_config(&serde_json::json!({"acceleration": "vulkan"}));
        let spec = super::LlamaServerPlugin.llm_capabilities();
        assert_eq!(
            spec.first().expect("one spec").resource_class,
            ResourceClass::Gpu { device: 0 }
        );
        super::config::set_config(&serde_json::json!({"acceleration": "cpu"}));
        let spec = super::LlamaServerPlugin.llm_capabilities();
        assert_eq!(
            spec.first().expect("one spec").resource_class,
            ResourceClass::Cpu
        );
        super::config::set_config(&serde_json::json!({}));
    }
}
