//! Local voice activity detection engine backed by Silero VAD ONNX (`ort`).
//!
//! The heavy native dependency is gated behind the `silero-vad` cargo feature.
//! When the feature is disabled, [`SileroVadFactory`] still registers but fails
//! fast with [`AudioProviderError::Init`] so the crate (and the workspace)
//! keeps compiling without the ONNX Runtime toolchain.

#[cfg(feature = "silero-vad")]
use crate::audio::VadEvent;
use crate::audio::{AudioProviderError, AudioProviderRegistry, VadEngine, VadFactory};

/// Engine name used in `ai.vad.provider` configuration.
pub const PROVIDER_NAME: &str = "silero";

/// Silero VAD operates on 16 kHz mono PCM.
#[cfg(feature = "silero-vad")]
const VAD_SAMPLE_RATE: u32 = 16_000;

/// Silero VAD v5 expects 512-sample (32 ms) chunks at 16 kHz.
#[cfg(feature = "silero-vad")]
const VAD_CHUNK_SAMPLES: usize = 512;

/// Silero VAD recurrent state dimension per layer (`h`/`c` are `[2, 1, 64]`).
#[cfg(feature = "silero-vad")]
const STATE_LEN: usize = 128;

/// Resolve the Silero VAD ONNX model path from configuration / environment.
///
/// Precedence: `ENE_AI__VAD__MODEL_PATH` env, then `VadConfig::model` when it
/// looks like a filesystem path, then a default cache location.
#[cfg(feature = "silero-vad")]
fn resolve_model_path(config: &ene_config::EneConfig) -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ENE_AI__VAD__MODEL_PATH")
        && !path.trim().is_empty()
    {
        return std::path::PathBuf::from(path.trim());
    }
    if let Ok(ai) = config.get_section::<crate::config::AiConfig>()
        && !ai.vad.model.trim().is_empty()
    {
        return std::path::PathBuf::from(ai.vad.model.trim());
    }
    crate::gguf::gguf_cache_dir().join("silero_vad.onnx")
}

/// Resolve the speech probability threshold from configuration.
#[cfg(feature = "silero-vad")]
fn resolve_threshold(config: &ene_config::EneConfig) -> f32 {
    config
        .get_section::<crate::config::AiConfig>()
        .map_or(0.5, |ai| ai.vad.threshold)
}

/// Local Silero VAD voice activity detection engine.
#[cfg(feature = "silero-vad")]
pub struct SileroVadEngine {
    session: ort::session::Session,
    /// Recurrent hidden state `h` (`[2, 1, 64]` flattened).
    h: Vec<f32>,
    /// Recurrent cell state `c` (`[2, 1, 64]` flattened).
    c: Vec<f32>,
    threshold: f32,
    speaking: bool,
}

#[cfg(feature = "silero-vad")]
impl SileroVadEngine {
    /// Load the Silero VAD ONNX model.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Init`] when the model file is missing or
    /// ONNX Runtime fails to build a session.
    pub fn open(model_path: &std::path::Path, threshold: f32) -> Result<Self, AudioProviderError> {
        if !model_path.is_file() {
            return Err(AudioProviderError::Init(format!(
                "Silero VAD ONNX model not found at {}",
                model_path.display()
            )));
        }
        let session = ort::session::Session::builder()
            .map_err(|e| AudioProviderError::Init(format!("ONNX session builder failed: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| AudioProviderError::Init(format!("ONNX session load failed: {e}")))?;
        tracing::info!(
            component = "SileroVad",
            path = %model_path.display(),
            threshold,
            "loaded Silero VAD model"
        );
        Ok(Self {
            session,
            h: vec![0.0; STATE_LEN],
            c: vec![0.0; STATE_LEN],
            threshold,
            speaking: false,
        })
    }

    /// Run a single 512-sample inference step, updating recurrent state.
    fn step(&mut self, chunk: &[f32; VAD_CHUNK_SAMPLES]) -> Result<f32, AudioProviderError> {
        let sr = vec![i64::from(VAD_SAMPLE_RATE)];
        let outputs = self
            .session
            .run(ort::inputs![
                "input" => ort::value::Tensor::from_array((
                    [1_i64, VAD_CHUNK_SAMPLES as i64],
                    chunk.to_vec(),
                ))
                .map_err(|e| AudioProviderError::Provider(format!("VAD input build failed: {e}")))?,
                "sr" => ort::value::Tensor::from_array(((), sr))
                .map_err(|e| AudioProviderError::Provider(format!("VAD sr build failed: {e}")))?,
                "h" => ort::value::Tensor::from_array((
                    [2_i64, 1, 64],
                    self.h.clone(),
                ))
                .map_err(|e| AudioProviderError::Provider(format!("VAD h build failed: {e}")))?,
                "c" => ort::value::Tensor::from_array((
                    [2_i64, 1, 64],
                    self.c.clone(),
                ))
                .map_err(|e| AudioProviderError::Provider(format!("VAD c build failed: {e}")))?,
            ])
            .map_err(|e| AudioProviderError::Provider(format!("VAD inference failed: {e}")))?;

        let prob = outputs
            .get("output")
            .ok_or_else(|| {
                AudioProviderError::Provider("Silero VAD produced no `output` tensor".to_string())
            })?
            .try_extract_tensor::<f32>()
            .map_err(|e| {
                AudioProviderError::Provider(format!("VAD output extraction failed: {e}"))
            })?
            .1
            .first()
            .copied()
            .unwrap_or(0.0);

        if let Some(hn) = outputs.get("hn")
            && let Ok((_shape, data)) = hn.try_extract_tensor::<f32>()
            && data.len() == STATE_LEN
        {
            self.h.copy_from_slice(data);
        }
        if let Some(cn) = outputs.get("cn")
            && let Ok((_shape, data)) = cn.try_extract_tensor::<f32>()
            && data.len() == STATE_LEN
        {
            self.c.copy_from_slice(data);
        }

        Ok(prob)
    }
}

#[cfg(feature = "silero-vad")]
impl VadEngine for SileroVadEngine {
    fn process_chunk(&mut self, pcm: &[f32]) -> VadEvent {
        // Silero requires exactly 512 samples; pad short chunks with silence
        // and truncate long ones so the recurrent step stays well-formed.
        let mut chunk = [0.0_f32; VAD_CHUNK_SAMPLES];
        for (dst, src) in chunk.iter_mut().zip(pcm.iter()) {
            *dst = *src;
        }

        let prob = match self.step(&chunk) {
            Ok(prob) => prob,
            Err(e) => {
                tracing::warn!(component = "SileroVad", error = %e, "VAD step failed");
                return VadEvent::Silence;
            }
        };

        let is_speech = prob >= self.threshold;
        match (self.speaking, is_speech) {
            (false, true) => {
                self.speaking = true;
                VadEvent::SpeechStart
            }
            (true, true) => VadEvent::SpeechContinue,
            (true, false) => {
                self.speaking = false;
                VadEvent::SpeechEnd
            }
            (false, false) => VadEvent::Silence,
        }
    }

    fn reset(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
        self.speaking = false;
    }

    fn name(&self) -> &str {
        PROVIDER_NAME
    }
}

/// Factory for the local Silero VAD engine.
pub struct SileroVadFactory;

#[cfg(feature = "silero-vad")]
impl SileroVadFactory {
    fn build(config: &ene_config::EneConfig) -> Result<Box<dyn VadEngine>, AudioProviderError> {
        let path = resolve_model_path(config);
        let threshold = resolve_threshold(config);
        let engine = SileroVadEngine::open(&path, threshold)?;
        Ok(Box::new(engine))
    }
}

#[cfg(not(feature = "silero-vad"))]
impl SileroVadFactory {
    fn build(_config: &ene_config::EneConfig) -> Result<Box<dyn VadEngine>, AudioProviderError> {
        Err(AudioProviderError::Init(
            "Silero VAD requested but ene-ai was built without the `silero-vad` feature"
                .to_string(),
        ))
    }
}

impl VadFactory for SileroVadFactory {
    fn provider_name(&self) -> &str {
        PROVIDER_NAME
    }

    fn create_engine(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn VadEngine>, AudioProviderError> {
        Self::build(config)
    }
}

/// Register the local Silero VAD factory at startup.
///
/// Registered unconditionally; the factory fails fast at `create_engine` time
/// when the `silero-vad` feature is disabled.
#[ctor::ctor(unsafe)]
fn register_silero_vad() {
    AudioProviderRegistry::register_vad(std::sync::Arc::new(SileroVadFactory));
}
