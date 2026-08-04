//! Local Kokoro-TTS (ONNX) provider plugin: capabilities, config, synthesis.

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use ene_ai::traits::TtsProvider;
use ene_plugin::prelude::*;
use serde_json::{Value, json};

use crate::config::{DEFAULT_PROFILE, KokoroConfig, ResolvedConfig};
use crate::wav;

/// Builds the ONNX-backed engine for a fully-resolved config. Injectable so
/// synthesis can be tested without a model file or ONNX Runtime.
pub(crate) type EngineBuilder =
    Arc<dyn Fn(&ResolvedConfig) -> Result<Arc<dyn TtsProvider>, PluginError> + Send + Sync>;

/// A loaded engine plus the config it was built from; a changed key forces
/// a rebuild. Only the last engine is kept — each holds a ~300 MB model, so
/// alternating per-request voices reload the model on every switch.
struct CachedEngine {
    key: crate::config::EngineKey,
    provider: Arc<dyn TtsProvider>,
}

/// TTS plugin serving the local Kokoro-82M ONNX model.
///
/// The static capability data (`tts_spec()` / `TTS_PROVIDER_KIND`) is
/// generated from the `#[provider(...)]` attribute; synthesis is
/// hand-written. The model loads lazily on first use and stays resident for
/// the process lifetime, mirroring the local-llm plugin's model handling.
#[derive(TtsPlugin)]
#[provider(
    kind = "kokoro",
    voices = "af_alloy, af_aoede, af_bella, af_heart, af_jessica, af_kore, af_nicole, af_nova, af_river, af_sarah, af_sky, am_adam, am_echo, am_eric, am_fenrir, am_liam, am_michael, am_onyx, am_puck, am_santa, bf_alice, bf_emma, bf_isabella, bf_lily, bm_daniel, bm_fable, bm_george, bm_lewis, ef_dora, em_alex, ff_siwis, hf_alpha, hf_beta, hm_omega, hm_psi, if_sara, im_nicola, jf_alpha, jf_gongitsune, jf_nezumi, jf_tebukuro, jm_kumo, pf_dora, pm_alex, pm_santa, zf_xiaobei, zf_xiaoni, zf_xiaoxiao, zf_xiaoyi, zm_yunjian, zm_yunxi, zm_yunxia, zm_yunyang",
    formats = "wav",
    // One ONNX session runs one job at a time; the host enforces the same
    // bound with admission control.
    concurrency = 1,
    queue_depth = 2,
)]
pub struct KokoroPlugin {
    /// `plugins.list.kokoro.profiles` map delivered at handshake; the
    /// `kokoro` profile carries the legacy `voices_path` slot.
    profiles: Mutex<Option<Value>>,
    /// Lazily-built engine, keyed by its resolved config.
    engine: Arc<Mutex<Option<CachedEngine>>>,
    build: EngineBuilder,
}

impl KokoroPlugin {
    /// Creates the plugin with the real ONNX-backed engine builder.
    #[must_use]
    pub fn new() -> Self {
        Self::with_builder(Arc::new(build_real))
    }

    pub(crate) fn with_builder(build: EngineBuilder) -> Self {
        Self {
            profiles: Mutex::new(None),
            engine: Arc::new(Mutex::new(None)),
            build,
        }
    }

    /// Rejects voice names the model cannot load before a doomed build pays
    /// the ONNX session load.
    fn ensure_known_voice(voice: &str) -> Result<(), PluginError> {
        if voice.is_empty() {
            return Ok(());
        }
        let available = KokoroPlugin::tts_spec().voices;
        if available.iter().any(|v| v == voice) {
            return Ok(());
        }
        Err(PluginError::provider(format!(
            "unknown Kokoro voice {voice:?}; available voices: {}",
            available.join(", ")
        )))
    }

    /// Returns the cached engine for `resolved`, or `None` on a miss.
    fn cached(&self, resolved: &ResolvedConfig) -> Option<Arc<dyn TtsProvider>> {
        let guard = self.engine.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .as_ref()
            .filter(|cached| cached.key == resolved.key())
            .map(|cached| Arc::clone(&cached.provider))
    }

    /// Returns the engine for `resolved`, building it on a cache miss or a
    /// key change. The ONNX session load can take seconds, so the build runs
    /// on the blocking pool; concurrent misses serialize on the mutex and
    /// re-check, so only one build per key happens.
    async fn engine(&self, resolved: &ResolvedConfig) -> Result<Arc<dyn TtsProvider>, PluginError> {
        if let Some(cached) = self.cached(resolved) {
            return Ok(cached);
        }
        let build = Arc::clone(&self.build);
        let engine = Arc::clone(&self.engine);
        let resolved = resolved.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = guard.as_ref()
                && cached.key == resolved.key()
            {
                return Ok(Arc::clone(&cached.provider));
            }
            let provider = (build)(&resolved)?;
            tracing::info!(
                component = "ene-plugin-kokoro",
                model = %resolved.model_path.display(),
                voices = %resolved.voices_path.display(),
                voice = %resolved.voice,
                "loaded Kokoro TTS engine"
            );
            *guard = Some(CachedEngine {
                key: resolved.key(),
                provider: Arc::clone(&provider),
            });
            Ok(provider)
        })
        .await
        .map_err(|e| PluginError::provider(format!("kokoro engine build task failed: {e}")))?
    }
}

impl Default for KokoroPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ene_plugin::ConfigurablePlugin for KokoroPlugin {
    /// Receives the per-profile config map (`plugins.list.kokoro.profiles`).
    fn set_profiles(&self, profiles: &Value) {
        *self.profiles.lock().unwrap_or_else(PoisonError::into_inner) = Some(profiles.clone());
    }

    /// Advertises the settings surface for `plugins.list.kokoro.config`.
    fn config_schema(&self) -> Option<Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "model_path": {
                    "type": "string",
                    "description": "Kokoro ONNX model file path (defaults to the shared models cache)"
                },
                "voices_path": {
                    "type": "string",
                    "description": "voices.bin path; falls back to plugins.list.kokoro.profiles.kokoro.voices_path, then the shared models cache"
                },
                "voice": {
                    "type": "string",
                    "description": "Default voice (e.g. af_heart, jf_alpha); a per-request voice overrides it; empty selects the first voice in voices.bin. Alternating voices reload the model on each switch."
                },
                "speed": {
                    "type": "number",
                    "minimum": 0.5,
                    "maximum": 2.0,
                    "default": 1.0,
                    "description": "Speech speed multiplier (0.5-2.0)"
                },
                "language": {
                    "type": "string",
                    "description": "G2P language: \"ja\" selects the Japanese kana rules; anything else uses the English rules"
                },
                "ort_dylib_path": {
                    "type": "string",
                    "description": "ONNX Runtime dynamic library path override (ort default resolution when unset). Fixed at process start: ONNX Runtime initializes once, so a change requires a restart"
                }
            }
        }))
    }
}

#[async_trait]
impl TtsPlugin for KokoroPlugin {
    fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
        vec![Self::tts_spec()]
    }

    async fn synthesize(
        &self,
        kind: &str,
        config: Value,
        text: String,
        voice: String,
        format: String,
    ) -> Result<Vec<u8>, PluginError> {
        if kind != Self::TTS_PROVIDER_KIND {
            return Err(PluginError::not_supported(format!("provider kind: {kind}")));
        }
        if format != "wav" {
            return Err(PluginError::provider(format!(
                "kokoro only emits wav audio; requested format: {format}"
            )));
        }
        if text.trim().is_empty() {
            return Err(PluginError::provider("cannot synthesize empty text"));
        }
        let parsed = KokoroConfig::from_value(&config)?;
        let profiles = self
            .profiles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let profile = profiles.as_ref().and_then(|p| p.get(DEFAULT_PROFILE));
        let resolved = parsed.resolve(&voice, profile);
        Self::ensure_known_voice(&resolved.voice)?;
        let provider = self.engine(&resolved).await?;
        synthesize_with(provider.as_ref(), &text).await
    }
}

fn build_real(resolved: &ResolvedConfig) -> Result<Arc<dyn TtsProvider>, PluginError> {
    ene_voice::local_tts::provider::open(
        &resolved.model_path,
        &resolved.voices_path,
        &resolved.voice,
        resolved.speed,
        resolved.language.clone(),
        resolved.ort_dylib_path.as_deref(),
    )
    .map(|engine| Arc::new(engine) as Arc<dyn TtsProvider>)
    .map_err(|e| PluginError::provider(format!("kokoro model init failed: {e}")))
}

/// Collects the provider's PCM chunks and wraps them in a WAV container.
async fn synthesize_with(provider: &dyn TtsProvider, text: &str) -> Result<Vec<u8>, PluginError> {
    let chunks = provider
        .synthesize(text)
        .await
        .map_err(|e| PluginError::provider(format!("kokoro synthesis failed: {e}")))?;
    let sample_rate = chunks.first().map_or(24_000, |chunk| chunk.sample_rate);
    let mut pcm = Vec::new();
    for chunk in chunks {
        pcm.extend_from_slice(&chunk.pcm);
    }
    wav::encode_wav(&pcm, sample_rate)
}
