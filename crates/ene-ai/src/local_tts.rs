//! Local text-to-speech provider backed by Kokoro ONNX (`ort`).
//!
//! The heavy native dependency is gated behind the `local-tts` cargo feature.
//! When the feature is disabled, [`LocalTtsProviderFactory`] still registers
//! but fails fast with [`AudioProviderError::Init`] so the crate (and the
//! workspace) keeps compiling without the ONNX Runtime toolchain.

#[cfg(feature = "local-tts")]
use crate::audio::TtsChunk;
use crate::audio::{AudioProviderError, AudioProviderRegistry, TtsProvider, TtsProviderFactory};
#[cfg(feature = "local-tts")]
use async_trait::async_trait;
#[cfg(feature = "local-tts")]
use std::pin::Pin;
#[cfg(feature = "local-tts")]
use tokio_stream::Stream;

/// Provider name used in `ai.tts.provider` configuration.
pub const PROVIDER_NAME: &str = "kokoro";

/// Kokoro ONNX emits 24 kHz mono PCM.
#[cfg(feature = "local-tts")]
const KOKORO_SAMPLE_RATE: u32 = 24_000;

/// Number of PCM samples yielded per streamed chunk (~0.25 s at 24 kHz).
#[cfg(feature = "local-tts")]
const CHUNK_SAMPLES: usize = 6_000;

/// Resolve the Kokoro ONNX model path from configuration / environment.
///
/// Precedence: `ENE_AI__TTS__MODEL_PATH` env, then `TtsConfig::model` when it
/// looks like a filesystem path, then a default cache location.
#[cfg(feature = "local-tts")]
fn resolve_model_path(config: &ene_config::EneConfig) -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ENE_AI__TTS__MODEL_PATH")
        && !path.trim().is_empty()
    {
        return std::path::PathBuf::from(path.trim());
    }
    if let Ok(ai) = config.get_section::<crate::config::AiConfig>()
        && !ai.tts.model.trim().is_empty()
    {
        return std::path::PathBuf::from(ai.tts.model.trim());
    }
    crate::gguf::gguf_cache_dir().join("kokoro.onnx")
}

/// Resolve the Kokoro `voices.bin` path from configuration / environment.
#[cfg(feature = "local-tts")]
fn resolve_voices_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ENE_AI__TTS__VOICES_PATH")
        && !path.trim().is_empty()
    {
        return std::path::PathBuf::from(path.trim());
    }
    crate::gguf::gguf_cache_dir().join("voices.bin")
}

/// Local Kokoro ONNX text-to-speech provider.
#[cfg(feature = "local-tts")]
pub struct LocalTtsProvider {
    session: std::sync::Arc<parking_lot::Mutex<ort::session::Session>>,
    voice: Vec<f32>,
    speed: f32,
}

#[cfg(feature = "local-tts")]
impl LocalTtsProvider {
    /// Load the Kokoro ONNX model and voice embedding.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Init`] when the model or voice file is
    /// missing, or ONNX Runtime fails to build a session.
    pub fn open(
        model_path: &std::path::Path,
        voices_path: &std::path::Path,
        speed: f32,
    ) -> Result<Self, AudioProviderError> {
        if !model_path.is_file() {
            return Err(AudioProviderError::Init(format!(
                "Kokoro ONNX model not found at {}",
                model_path.display()
            )));
        }
        let session = ort::session::Session::builder()
            .map_err(|e| AudioProviderError::Init(format!("ONNX session builder failed: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| AudioProviderError::Init(format!("ONNX session load failed: {e}")))?;

        let voice = load_voice_embedding(voices_path)?;

        tracing::info!(
            component = "LocalTts",
            model = %model_path.display(),
            voices = %voices_path.display(),
            "loaded Kokoro ONNX model"
        );
        Ok(Self {
            session: std::sync::Arc::new(parking_lot::Mutex::new(session)),
            voice,
            speed,
        })
    }

    /// Run Kokoro inference and return the full PCM buffer (blocking).
    ///
    /// Tokenization is intentionally minimal (byte-level); a production
    /// deployment should front this with a grapheme-to-phoneme tokenizer.
    fn run_inference(
        session: &mut ort::session::Session,
        voice: &[f32],
        text: &str,
        speed: f32,
    ) -> Result<Vec<f32>, AudioProviderError> {
        let tokens: Vec<i64> = text.bytes().map(i64::from).collect();
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let seq_len = tokens.len();

        let style = voice.to_vec();
        let outputs = session
            .run(ort::inputs![
                "input_ids" => ort::value::Tensor::from_array((
                    [1_i64, seq_len as i64],
                    tokens,
                ))
                .map_err(|e| AudioProviderError::Provider(format!("ONNX input build failed: {e}")))?,
                "style" => ort::value::Tensor::from_array((
                    [1_i64, style.len() as i64],
                    style,
                ))
                .map_err(|e| AudioProviderError::Provider(format!("ONNX style build failed: {e}")))?,
                "speed" => ort::value::Tensor::from_array(([1_i64], vec![speed]))
                .map_err(|e| AudioProviderError::Provider(format!("ONNX speed build failed: {e}")))?,
            ])
            .map_err(|e| AudioProviderError::Provider(format!("ONNX inference failed: {e}")))?;

        let output = outputs.get("output").ok_or_else(|| {
            AudioProviderError::Provider(
                "Kokoro ONNX model produced no `output` tensor".to_string(),
            )
        })?;
        let (_shape, data) = output.try_extract_tensor::<f32>().map_err(|e| {
            AudioProviderError::Provider(format!("Kokoro ONNX output extraction failed: {e}"))
        })?;
        Ok(data.to_vec())
    }
}

/// Load a single 256-dim voice embedding from `voices.bin`.
///
/// `voices.bin` is a flat little-endian `f32` array of stacked voice vectors;
/// the first voice is used by default.
#[cfg(feature = "local-tts")]
fn load_voice_embedding(voices_path: &std::path::Path) -> Result<Vec<f32>, AudioProviderError> {
    const VOICE_DIM: usize = 256;
    if !voices_path.is_file() {
        return Err(AudioProviderError::Init(format!(
            "Kokoro voices.bin not found at {}",
            voices_path.display()
        )));
    }
    let bytes = std::fs::read(voices_path)
        .map_err(|e| AudioProviderError::Init(format!("failed to read voices.bin: {e}")))?;
    let floats: Vec<f32> = bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let mut le = [0_u8; std::mem::size_of::<f32>()];
            le.copy_from_slice(chunk);
            f32::from_le_bytes(le)
        })
        .collect();
    if floats.len() < VOICE_DIM {
        return Err(AudioProviderError::Init(format!(
            "voices.bin too small: {} floats, expected at least {VOICE_DIM}",
            floats.len()
        )));
    }
    Ok(floats.into_iter().take(VOICE_DIM).collect())
}

#[cfg(feature = "local-tts")]
#[async_trait]
impl TtsProvider for LocalTtsProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    > {
        let session = std::sync::Arc::clone(&self.session);
        let voice = self.voice.clone();
        let speed = self.speed;
        let text = text.to_string();

        // ONNX inference is CPU-bound and blocking; run it off the runtime.
        let pcm = tokio::task::spawn_blocking(move || {
            let mut guard = session.lock();
            Self::run_inference(&mut guard, &voice, &text, speed)
        })
        .await
        .map_err(|e| AudioProviderError::Provider(format!("kokoro task join error: {e}")))??;

        // Slice the contiguous PCM buffer into fixed-size stream chunks.
        let chunks: Vec<Result<TtsChunk, AudioProviderError>> = pcm
            .chunks(CHUNK_SAMPLES)
            .map(|slice| {
                Ok(TtsChunk {
                    pcm: slice.to_vec(),
                    sample_rate: KOKORO_SAMPLE_RATE,
                })
            })
            .collect();
        Ok(Box::pin(tokio_stream::iter(chunks)))
    }
}

/// Factory for the local Kokoro ONNX TTS provider.
pub struct LocalTtsProviderFactory;

#[cfg(feature = "local-tts")]
impl LocalTtsProviderFactory {
    fn build(config: &ene_config::EneConfig) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        let ai = config
            .get_section::<crate::config::AiConfig>()
            .map_err(|e| AudioProviderError::Init(format!("failed to parse AI config: {e}")))?;
        let model_path = resolve_model_path(config);
        let voices_path = resolve_voices_path();
        let provider = LocalTtsProvider::open(&model_path, &voices_path, ai.tts.speed)?;
        Ok(Box::new(provider))
    }
}

#[cfg(not(feature = "local-tts"))]
impl LocalTtsProviderFactory {
    fn build(_config: &ene_config::EneConfig) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        Err(AudioProviderError::Init(
            "local TTS requested but ene-ai was built without the `local-tts` feature".to_string(),
        ))
    }
}

impl TtsProviderFactory for LocalTtsProviderFactory {
    fn provider_name(&self) -> &str {
        PROVIDER_NAME
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        Self::build(config)
    }
}

/// Register the local Kokoro TTS factory at startup.
///
/// Registered unconditionally; the factory fails fast at `create_provider`
/// time when the `local-tts` feature is disabled.
#[ctor::ctor(unsafe)]
fn register_local_tts() {
    AudioProviderRegistry::register_tts(std::sync::Arc::new(LocalTtsProviderFactory));
}
