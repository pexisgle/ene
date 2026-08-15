//! Local whisper.cpp STT provider plugin: capabilities, config, transcription.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ene_ai::traits::SttProvider;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::config::WhisperConfig;
use crate::wav;

/// Builds the whisper.cpp-backed engine for a fully-resolved config.
/// Injectable so transcription can be tested without a model file.
pub(crate) type EngineBuilder = Arc<
    dyn Fn(&WhisperConfig, &str, Option<String>) -> Result<Arc<dyn SttProvider>, PluginError>
        + Send
        + Sync,
>;

/// A loaded engine plus the config it was built from; a changed key forces
/// a rebuild. Only the last engine is kept — a multi-hundred-megabyte model
/// stays resident for the process lifetime.
struct CachedEngine {
    key: EngineKey,
    provider: Arc<dyn SttProvider>,
}

/// Everything a loaded whisper model depends on.
#[derive(Debug, Clone, PartialEq)]
struct EngineKey {
    model_path: std::path::PathBuf,
    language: Option<String>,
}

/// STT plugin serving the local whisper.cpp engine.
///
/// The static capability data (`stt_spec()` / `STT_PROVIDER_KIND`) comes
/// from the `#[provider(...)]` attribute; the model list is deliberately
/// empty because model selection is path-based (the `model` config value is
/// a path fallback inside the provider-owned config blob).
///
/// The `whisper-runner@1` capability declaration is hand-written because the
/// derive only emits `provides()` for `LlmPlugin`.
#[derive(SttPlugin)]
#[provider(
    kind = "whisper",
    formats = "wav",
    // One whisper.cpp model runs one job at a time; the host enforces the
    // same bound with admission control.
    concurrency = 1,
    queue_depth = 2,
)]
pub struct WhisperPlugin {
    /// Lazily-built engine, keyed by its resolved config.
    engine: Arc<Mutex<Option<CachedEngine>>>,
    build: EngineBuilder,
    /// Delivered config from `set_config` (handshake / live `SetConfig`),
    /// canonical over the per-request blob (which may predate an artifact
    /// injection).
    delivered: std::sync::Mutex<Option<WhisperConfig>>,
}

impl WhisperPlugin {
    /// Creates the plugin with the real whisper.cpp-backed engine builder.
    #[must_use]
    pub fn new() -> Self {
        Self::with_builder(Arc::new(build_real))
    }

    pub(crate) fn with_builder(build: EngineBuilder) -> Self {
        Self {
            engine: Arc::new(Mutex::new(None)),
            build,
            delivered: std::sync::Mutex::new(None),
        }
    }

    /// The canonical provider config: the delivered `set_config` blob, or
    /// the per-request blob when the host never delivered one.
    fn effective_config(&self, request: &Value) -> Result<WhisperConfig, PluginError> {
        let delivered = self
            .delivered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match delivered {
            Some(config) => Ok(config),
            None => WhisperConfig::from_value(request),
        }
    }

    /// Returns the engine for `key`, building it on a cache miss or a key
    /// change. Model load can take seconds, so the build runs on the
    /// blocking pool; concurrent misses serialize on the mutex and re-check.
    async fn engine(
        &self,
        config: &WhisperConfig,
        request_model: String,
        language: Option<String>,
    ) -> Result<Arc<dyn SttProvider>, PluginError> {
        let key = EngineKey {
            model_path: config.resolve_model_path(&request_model),
            language: language.clone(),
        };
        {
            let guard = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = guard.as_ref()
                && cached.key == key
            {
                return Ok(Arc::clone(&cached.provider));
            }
        }
        let build = Arc::clone(&self.build);
        let engine = Arc::clone(&self.engine);
        let config = config.clone();
        let resolved = (key.model_path.clone(), request_model, language);
        let provider =
            tokio::task::spawn_blocking(move || -> Result<Arc<dyn SttProvider>, PluginError> {
                let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(cached) = guard.as_ref()
                    && cached.key.model_path == resolved.0
                    && cached.key.language == resolved.2
                {
                    return Ok(Arc::clone(&cached.provider));
                }
                let provider = (build)(&config, &resolved.1, resolved.2.clone())?;
                tracing::info!(
                    component = "ene-plugin-whisper",
                    model = %resolved.0.display(),
                    language = ?resolved.2,
                    "loaded whisper.cpp engine"
                );
                *guard = Some(CachedEngine {
                    key: EngineKey {
                        model_path: resolved.0,
                        language: resolved.2,
                    },
                    provider: Arc::clone(&provider),
                });
                Ok(provider)
            })
            .await
            .map_err(|e| {
                PluginError::provider(format!("whisper engine build task failed: {e}"))
            })??;
        Ok(provider)
    }
}

impl Default for WhisperPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ene_plugin::ConfigurablePlugin for WhisperPlugin {
    fn set_config(&self, config: &Value) {
        if let Ok(config) = WhisperConfig::from_value(config) {
            *self
                .delivered
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(config);
        }
    }

    /// Advertises the settings surface for `plugins.list.whisper.config`.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "whisper GGUF model name (e.g. ggml-small.bin); used as a path fallback when model_path is unset",
                    "x-ene-ui": { "order": 0, "impact": "runtime_reload", "label_key": "provider-whisper-model-label", "description_key": "provider-whisper-model-desc" }
                },
                "language": {
                    "type": "string",
                    "description": "Language hint (e.g. ja, en); empty = auto-detect",
                    "x-ene-ui": { "order": 1, "impact": "runtime_reload", "label_key": "provider-whisper-language-label", "description_key": "provider-whisper-language-desc" }
                },
                "model_path": {
                    "type": "string",
                    "description": "whisper.cpp GGUF model file path (defaults to the shared models cache)",
                    "x-ene-ui": { "order": 2, "impact": "plugin_restart", "label_key": "provider-whisper-model-path-label", "description_key": "provider-whisper-model-path-desc" }
                },
                "mode": {
                    "type": "string",
                    "enum": ["auto", "sidecar", "in-process"],
                    "default": "auto",
                    "description": "Engine mode: auto uses the whisper-server sidecar when a server binary is configured, otherwise the in-process engine",
                    "x-ene-ui": { "order": 3, "impact": "plugin_restart", "label_key": "provider-whisper-mode-label", "description_key": "provider-whisper-mode-desc" }
                },
                "server_path": {
                    "type": "string",
                    "description": "Path to the whisper-server executable (host-injected from the artifact catalog when installed)",
                    "x-ene-ui": { "order": 4, "impact": "plugin_restart", "label_key": "provider-whisper-server-path-label", "description_key": "provider-whisper-server-path-desc" }
                },
                "server_args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra command-line arguments passed to whisper-server",
                    "x-ene-ui": { "order": 5, "impact": "plugin_restart", "label_key": "provider-whisper-server-args-label", "description_key": "provider-whisper-server-args-desc" }
                },
                "startup_timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How long to wait for whisper-server /health after spawning",
                    "x-ene-ui": { "order": 6, "impact": "plugin_restart", "label_key": "provider-whisper-startup-timeout-label", "description_key": "provider-whisper-startup-timeout-desc" }
                }
            }
        }))
    }
}

#[async_trait]
impl SttPlugin for WhisperPlugin {
    fn stt_capabilities(&self) -> Vec<SttProviderSpec> {
        vec![Self::stt_spec()]
    }

    async fn transcribe(
        &self,
        kind: &str,
        config: Value,
        audio_data: Vec<u8>,
        format: String,
    ) -> Result<PluginTranscription, PluginError> {
        if kind != Self::STT_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        if format != "wav" {
            return Err(PluginError::provider(format!(
                "whisper only accepts wav audio; requested format: {format}"
            )));
        }
        if audio_data.is_empty() {
            return Err(PluginError::provider(
                "cannot transcribe empty audio".to_string(),
            ));
        }
        let config = self.effective_config(&config)?;
        // `model` / `language` live in the provider-owned config blob
        // (`plugins.list.whisper.config`); the host forwards it verbatim.
        let request_model = config.model.clone().unwrap_or_default();
        let language = config.language.clone();
        if config.wants_sidecar() {
            let model_path = config.resolve_model_path(&request_model);
            let state = crate::server::ensure_sidecar(&config, &model_path).await?;
            let result = crate::server::transcribe(&state, audio_data, language.as_deref()).await?;
            return Ok(PluginTranscription {
                text: result.text,
                language: result.language,
            });
        }
        let provider = self.engine(&config, request_model, language).await?;
        let decoded = wav::decode_wav(&audio_data)?;
        provider
            .transcribe(&decoded.pcm, decoded.sample_rate)
            .await
            .map(|result| PluginTranscription {
                text: result.text,
                language: result.language,
            })
            .map_err(|e| PluginError::provider(format!("whisper transcription failed: {e}")))
    }
}

fn build_real(
    config: &WhisperConfig,
    request_model: &str,
    language: Option<String>,
) -> Result<Arc<dyn SttProvider>, PluginError> {
    ene_voice::local_stt::open(&config.resolve_model_path(request_model), language)
        .map(|engine| Arc::new(engine) as Arc<dyn SttProvider>)
        .map_err(|e| PluginError::provider(format!("whisper model init failed: {e}")))
}

/// Capabilities this plugin provides to other plugins: the whisper.cpp STT
/// runtime shared across STT consumers.
#[must_use]
pub fn provides() -> Vec<CapabilityRef> {
    ["whisper-runner@1"]
        .into_iter()
        .filter_map(|c| CapabilityRef::parse(c).ok())
        .collect()
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use super::*;

    type ModelLoadLog = Arc<Mutex<Vec<(String, Option<String>)>>>;

    fn fake_builder(text: String) -> EngineBuilder {
        Arc::new(move |_config, _model, _language| {
            let text = text.clone();
            Ok(Arc::new(FakeStt { text }) as Arc<dyn SttProvider>)
        })
    }

    /// Builder that records the `(model, language)` it was asked to load.
    fn recording_builder(seen: ModelLoadLog) -> EngineBuilder {
        Arc::new(move |_config, model, language| {
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((model.to_string(), language));
            Ok(Arc::new(FakeStt {
                text: "ok".to_string(),
            }) as Arc<dyn SttProvider>)
        })
    }

    struct FakeStt {
        text: String,
    }

    #[async_trait]
    impl SttProvider for FakeStt {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn transcribe(
            &self,
            _pcm: &[f32],
            _sample_rate: u32,
        ) -> Result<ene_ai::SttResult, ene_ai::AudioProviderError> {
            Ok(ene_ai::SttResult {
                text: self.text.clone(),
                language: None,
                duration_secs: 0.0,
            })
        }
    }

    #[tokio::test]
    async fn transcribe_decodes_wav_and_returns_text() {
        let plugin = WhisperPlugin::with_builder(fake_builder("hello world".into()));
        let bytes = crate::wav::decode_wav_test_fixture();
        let text = plugin
            .transcribe(
                WhisperPlugin::STT_PROVIDER_KIND,
                json!({"model_path": "/data/whisper.gguf", "model": "small.gguf", "language": "en"}),
                bytes,
                "wav".into(),
            )
            .await
            .expect("transcribe");
        assert_eq!(text.text, "hello world");
        assert_eq!(text.language, None);
    }

    /// The config delivered at handshake / live `SetConfig` is canonical: a
    /// stale request blob (raw persisted config) must not override the
    /// delivered model/language on the transcribe path.
    #[tokio::test]
    async fn delivered_config_beats_stale_request_blob() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let plugin = WhisperPlugin::with_builder(recording_builder(Arc::clone(&seen)));
        plugin.set_config(&json!({
            "model": "delivered.gguf",
            "language": "ja",
            "mode": "in-process"
        }));

        plugin
            .transcribe(
                WhisperPlugin::STT_PROVIDER_KIND,
                json!({ "model": "stale.gguf", "language": "en", "mode": "in-process" }),
                crate::wav::decode_wav_test_fixture(),
                "wav".into(),
            )
            .await
            .expect("transcribe");

        let seen = seen.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(
            seen.as_slice(),
            &[("delivered.gguf".to_string(), Some("ja".to_string()))],
            "the delivered config drives the engine build, not the stale blob"
        );
    }

    #[tokio::test]
    async fn rejects_wrong_kind_format_and_empty_audio() {
        let plugin = WhisperPlugin::with_builder(fake_builder(String::new()));
        let err = plugin
            .transcribe("other", json!({}), vec![0; 44], "wav".into())
            .await
            .expect_err("wrong kind");
        assert!(err.to_string().contains("not supported"));
        let err = plugin
            .transcribe(
                WhisperPlugin::STT_PROVIDER_KIND,
                json!({}),
                vec![0; 44],
                "ogg".into(),
            )
            .await
            .expect_err("wrong format");
        assert!(err.to_string().contains("only accepts wav"));
        let err = plugin
            .transcribe(
                WhisperPlugin::STT_PROVIDER_KIND,
                json!({}),
                Vec::new(),
                "wav".into(),
            )
            .await
            .expect_err("empty audio");
        assert!(err.to_string().contains("empty audio"));
    }

    #[test]
    fn capability_declarations_are_valid() {
        assert_eq!(
            provides(),
            ["whisper-runner@1"]
                .into_iter()
                .filter_map(|c| CapabilityRef::parse(c).ok())
                .collect::<Vec<_>>()
        );
    }
}
