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
/// a path fallback, forwarded per request from `ai.stt.model`).
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
    /// Advertises the settings surface for `plugins.list.whisper.config`.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "model_path": {
                    "type": "string",
                    "description": "whisper.cpp GGUF model file path (defaults to the shared models cache; ai.stt.model is used as a path fallback)"
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
        let blob = config.clone();
        let config = WhisperConfig::from_value(&config)?;
        // The host forwards `ai.stt.model` and `ai.stt.language` per request
        // (mirroring the TTS adapter's `ai.tts.voice` forwarding), so the
        // blob carries them alongside `plugins.list.whisper.config`.
        let request_model = blob
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        let language = blob
            .get("language")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string);
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

    fn fake_builder(text: String) -> EngineBuilder {
        Arc::new(move |_config, _model, _language| {
            let text = text.clone();
            Ok(Arc::new(FakeStt { text }) as Arc<dyn SttProvider>)
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
