//! Silero VAD ONNX engine (`ort`), consumed by the `onnx` provider plugin.
//!
//! The heavy native dependency is gated behind the `silero-vad` cargo
//! feature; the plugin crate is what enables it.
//!
//! # Why `SileroVadEngine` is not an `ene_infer::LocalModel`
//!
//! [`ene_infer::LocalModel`] exists to remove a specific bug class from the
//! whisper and Kokoro providers: a `spawn_blocking`/`block_in_place` call or
//! a mutex take/put-back pattern smuggled past `async fn`. [`SileroVadEngine`]
//! does not need that treatment:
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
//! In short: this is the one local provider that never had the bug class
//! [`ene_infer`] exists to remove; `process_chunk` is plain synchronous,
//! exclusive ownership of blocking work. If a future change makes it slow
//! enough to need a dedicated worker thread and cooperative timeouts, this
//! doc comment is the note that it stopped being the exception.
#[cfg(feature = "silero-vad")]
use ene_ai::VadEvent;
use ene_ai::{AudioProviderError, VadEngine};

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
/// escalates to a hard error.
#[cfg(feature = "silero-vad")]
const MAX_CONSECUTIVE_FAILURES: u32 = 5;

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
    /// Count of consecutive inference failures.
    consecutive_failures: u32,
}

#[cfg(feature = "silero-vad")]
impl SileroVadEngine {
    /// Load the Silero VAD ONNX model.
    ///
    /// ONNX Runtime is initialized exactly once per process using
    /// `ort_dylib_path` when provided. The `threshold` is expected to already
    /// be clamped to `[0.0, 1.0]` by the caller (the onnx plugin's config
    /// validation).
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
        // Enforce the frame-size contract: Silero requires exactly 512
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
