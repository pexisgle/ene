//! Local GGUF provider plugin: capability skeleton.
//!
//! Implements [`ConfigurablePlugin`] (mmproj / acceleration config and
//! per-model profiles) and declares the provider capabilities the inference
//! core serves in a later slice. The [`LlmPlugin`] and [`EmbedPlugin`] action
//! handlers are stubs returning `NotSupported` until that core lands.

use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

/// Error message for inference actions until the llama.cpp core lands.
const INFERENCE_NOT_IMPLEMENTED: &str =
    "local inference is not implemented in this slice; the llama.cpp core lands in Slice C";

/// Configuration delivered by the host at handshake time
/// (`plugins.list.llama-cpp.config`), stored per process.
///
/// `Mutex` (rather than `OnceLock`) so tests can reset it between cases; in
/// production the handshake is a one-shot and reconnects resend the same
/// blob, so last-writer-wins is equivalent.
static PLUGIN_CONFIG: Mutex<Option<Value>> = Mutex::new(None);

/// Per-profile configuration (`plugins.list.llama-cpp.profiles`), stored per
/// process. Profile *selection* is plugin-owned and starts in Slice C.
static PLUGIN_PROFILES: Mutex<Option<Value>> = Mutex::new(None);

/// Local GGUF inference provider plugin (llama.cpp).
///
/// The static capability data (`llm_spec()` / `LLM_PROVIDER_KIND` /
/// `provides()`) is generated from the `#[provider(...)]` attribute; the
/// async handlers below stay hand-written stubs until Slice C.
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
    /// Receives the plugin configuration blob from the host at handshake
    /// time (`plugins.list.llama-cpp.config`).
    fn set_config(&self, config: &Value) {
        *PLUGIN_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(config.clone());
    }

    /// Receives the per-model profile map (`plugins.list.llama-cpp.profiles`).
    fn set_profiles(&self, profiles: &Value) {
        *PLUGIN_PROFILES
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(profiles.clone());
    }

    /// Advertises the config schema. Profile shape (`url` / `quantization` /
    /// `model_path` / `gpu_layers` per model) is delivered via `set_profiles`
    /// and documented in `docs/configuration.md`; the host treats profiles as
    /// opaque.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "mmproj_url": {
                    "type": "string",
                    "description": "HTTPS URL for the multimodal projector (mmproj) GGUF"
                },
                "mmproj_path": {
                    "type": "string",
                    "description": "Optional filesystem path for mmproj (skips download when non-empty)"
                },
                "acceleration": {
                    "type": "string",
                    "enum": ["auto", "cpu", "vulkan", "cuda"],
                    "description": "Preferred acceleration backend for llama.cpp"
                }
            }
        }))
    }
}

#[async_trait]
impl LlmPlugin for LocalLlmPlugin {
    fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
        vec![Self::llm_spec()]
    }

    async fn create_chat_stream(
        &self,
        kind: &str,
        _config: Value,
        _model: String,
        _max_tokens: Option<u32>,
        _messages: Vec<Value>,
        _tools: Vec<Value>,
    ) -> Result<PluginStream, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        Err(PluginError::not_supported(INFERENCE_NOT_IMPLEMENTED))
    }

    async fn chat_completion(
        &self,
        kind: &str,
        _config: Value,
        _model: String,
        _max_tokens: Option<u32>,
        _messages: Vec<Value>,
        _json_schema: Option<Value>,
    ) -> Result<PluginCompletion, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        Err(PluginError::not_supported(INFERENCE_NOT_IMPLEMENTED))
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
        _model: String,
        _dimensions: Option<u32>,
        _items: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, PluginError> {
        if kind != Self::LLM_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        Err(PluginError::not_supported(INFERENCE_NOT_IMPLEMENTED))
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

    use ene_plugin::{ConfigurablePlugin, PluginDispatch};
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
    /// inference stub error.
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

        // Capability declaration: kind `"local"` provider spec + embedding
        // kind + the Slice A `provides` contract.
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

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::CreateChatStream {
                request_id: "req-chat".to_string(),
                provider_kind: "local".to_string(),
                provider_config: serde_json::json!({}),
                model: "gemma-4-e4b".to_string(),
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
            panic!("expected StreamError for stub, got {resp:?}");
        };
        assert!(
            message.contains("not implemented"),
            "stub must explain the slice boundary: {message}"
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
        assert_eq!(super::LocalLlmPlugin::LLM_PROVIDER_KIND, "local");
        assert_eq!(
            super::LocalLlmPlugin::provides(),
            vec![
                CapabilityRef::parse("llm/chat@1").expect("static capability"),
                CapabilityRef::parse("embed@1").expect("static capability"),
                CapabilityRef::parse("gguf-runner@1").expect("static capability"),
            ]
        );
    }
}
