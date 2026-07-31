//! Local voice activity detection engine backed by Silero VAD ONNX (`ort`).
//!
//! The heavy native dependency is gated behind the `silero-vad` cargo feature.
//! When the feature is disabled, [`SileroVadFactory`] still registers but fails
//! fast with [`AudioProviderError::Init`] so the crate (and the workspace)
//! keeps compiling without the ONNX Runtime toolchain.
//!
//! # Deliberately *not* migrated to `ene_infer::LocalModel`
//!
//! Stage 4 migrated `local_stt.rs` (whisper.cpp) and `local_tts.rs` (Kokoro
//! ONNX) onto [`ene_infer::LocalModel`] because both used to smuggle a
//! `spawn_blocking`/`block_in_place` call or a mutex take/put-back pattern
//! past `async fn` — the exact class of bug [`ene_infer`] exists to remove.
//! [`SileroVadEngine`] was assessed for the same treatment and left alone,
//! on purpose:
//!
//! - [`VadEngine::process_chunk`] is already a plain, synchronous `&mut self`
//!   method (see `ene-ai`'s `traits.rs`) — there is no `async fn` here to
//!   have gotten concurrency wrong in the first place, and no
//!   `spawn_blocking`/`block_in_place`/mutex-take pattern to remove.
//! - Callers already own a `Box<dyn VadEngine>` exclusively and call
//!   `process_chunk` synchronously, once per fixed-size 512-sample frame, in
//!   the same thread that owns it. That is already exactly the invariant
//!   [`ene_infer::LocalModel`] exists to provide (a model owned by exactly
//!   one caller, never invoked concurrently with itself) — nothing here
//!   would change by adding a worker thread and a queue in front of it.
//! - Wrapping it would add a queue, a channel round-trip, and an async
//!   `submit` call *per 32ms audio frame* in what is currently a direct,
//!   zero-allocation-on-the-hot-path function call. For a VAD loop meant to
//!   keep up with a live microphone stream, that is pure overhead with no
//!   correctness upside — [`ene_infer::EngineConfig::job_timeout`] and
//!   cooperative cancellation solve problems (a wedged multi-second
//!   inference call, an unbounded queue) that a 512-sample Silero step
//!   simply does not have.
//!
//! In short: this file is the one of the four local providers that never had
//! the bug class this stage exists to fix, and that is not a coincidence —
//! its author already gave the model plain synchronous, exclusive ownership
//! instead of reaching for `Arc<Mutex<_>>` around blocking work. Nothing
//! here needed migrating; if a future change makes `process_chunk` slow
//! enough to need a dedicated worker thread and cooperative timeouts, this
//! doc comment is the note that it stopped being the exception.
#[cfg(feature = "silero-vad")]
use ene_ai::VadEvent;
use ene_ai::{AudioProviderError, AudioProviderRegistry, VadEngine, VadFactory};

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

/// Number of consecutive inference failures before [`SileroVadEngine`]
/// escalates to a hard error (M13).
#[cfg(feature = "silero-vad")]
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

/// Resolve the Silero VAD ONNX model path from configuration.
///
/// Precedence: `VadConfig::model_path` when non-empty, then `VadConfig::model`
/// when non-empty, then a default cache location. Environment overrides are
/// handled by the config system (`ENE_AI__VAD__MODEL_PATH`).
#[cfg(feature = "silero-vad")]
fn resolve_model_path(ai: &ene_ai::AiConfig) -> std::path::PathBuf {
    if let Some(path) = ai
        .vad
        .model_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    if !ai.vad.model.trim().is_empty() {
        return std::path::PathBuf::from(ai.vad.model.trim());
    }
    ene_config::models_dir()
        .join("gguf")
        .join("silero_vad.onnx")
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
    /// Count of consecutive inference failures (M13 escalation).
    consecutive_failures: u32,
}

#[cfg(feature = "silero-vad")]
impl SileroVadEngine {
    /// Load the Silero VAD ONNX model.
    ///
    /// ONNX Runtime is initialized exactly once per process (C3) using
    /// `ort_dylib_path` when provided. The `threshold` is expected to already
    /// be clamped to `[0.0, 1.0]` by the caller (see
    /// [`AiConfig::resolve_vad`](crate::config::AiConfig::resolve_vad)).
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Init`] when the model file is missing or
    /// ONNX Runtime fails to build a session.
    pub fn open(
        model_path: &std::path::Path,
        threshold: f32,
        ort_dylib_path: Option<&str>,
    ) -> Result<Self, AudioProviderError> {
        crate::ort_init::ensure_ort_init(ort_dylib_path)?;

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
            consecutive_failures: 0,
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
    fn frame_size(&self) -> usize {
        VAD_CHUNK_SAMPLES
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<VadEvent, AudioProviderError> {
        // Enforce the frame-size contract (H11): Silero requires exactly 512
        // samples per step.
        if pcm.len() != self.frame_size() {
            return Err(AudioProviderError::Provider(format!(
                "Silero VAD expects {} samples per chunk, got {}",
                self.frame_size(),
                pcm.len()
            )));
        }

        let mut chunk = [0.0_f32; VAD_CHUNK_SAMPLES];
        chunk.copy_from_slice(pcm);

        let prob = match self.step(&chunk) {
            Ok(prob) => {
                self.consecutive_failures = 0;
                prob
            }
            Err(e) => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    return Err(AudioProviderError::Provider(format!(
                        "Silero VAD failed {MAX_CONSECUTIVE_FAILURES} consecutive steps: {e}"
                    )));
                }
                tracing::warn!(
                    component = "SileroVad",
                    error = %e,
                    consecutive_failures = self.consecutive_failures,
                    "VAD step failed; treating as silence"
                );
                return Ok(VadEvent::Silence);
            }
        };

        let is_speech = prob >= self.threshold;
        Ok(match (self.speaking, is_speech) {
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
        })
    }

    fn reset(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
        self.speaking = false;
        self.consecutive_failures = 0;
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
        let ai = config
            .get_section::<ene_ai::AiConfig>()
            .map_err(|e| AudioProviderError::Init(format!("failed to parse AI config: {e}")))?;
        let resolved = ai.resolve_vad().ok_or_else(|| {
            AudioProviderError::Init(
                "VAD engine is disabled (ai.vad.provider = \"none\")".to_string(),
            )
        })?;
        let path = resolve_model_path(&ai);
        let engine =
            SileroVadEngine::open(&path, resolved.threshold, ai.ort_dylib_path.as_deref())?;
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

/// Register the local Silero VAD factory.
///
/// Called once from [`crate::register_providers`] during runtime bootstrap,
/// rather than via a `#[ctor::ctor]` that would run before `main` (and before
/// `tracing` is initialized). Registered unconditionally; the factory fails
/// fast at `create_engine` time when the `silero-vad` feature is disabled.
pub(crate) fn register() {
    AudioProviderRegistry::register_vad(std::sync::Arc::new(SileroVadFactory));
    tracing::debug!(
        component = "ene-voice",
        provider = PROVIDER_NAME,
        "registered local VAD engine factory"
    );
}
