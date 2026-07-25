//! Local speech-to-text provider backed by `whisper.cpp` (`whisper-rs`).
//!
//! The heavy native dependency is gated behind the `local-stt` cargo feature.
//! When the feature is disabled, [`LocalSttProviderFactory`] still registers
//! but fails fast with [`AudioProviderError::Init`] so the crate (and the
//! workspace) keeps compiling without the whisper.cpp toolchain.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "FIR filter and resampler use bounded counter arithmetic over PCM buffers"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "resampler indexes into bounded PCM buffers with clamped positions"
)]

#[cfg(feature = "local-stt")]
use async_trait::async_trait;
#[cfg(feature = "local-stt")]
use ene_ai::SttResult;
use ene_ai::{AudioProviderError, AudioProviderRegistry, SttProvider, SttProviderFactory};

/// Provider name used in `ai.stt.provider` configuration.
pub const PROVIDER_NAME: &str = "whisper";

/// whisper.cpp operates on 16 kHz mono PCM.
#[cfg(feature = "local-stt")]
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Number of taps for the anti-aliasing FIR low-pass filter (M15).
#[cfg(feature = "local-stt")]
const FIR_TAPS: usize = 16;

/// Resolve the whisper GGUF model path from configuration.
///
/// Precedence: `SttConfig::model_path` when non-empty, then `SttConfig::model`
/// when non-empty, then a default cache location. Environment overrides are
/// handled by the config system (`ENE_AI__STT__MODEL_PATH`).
#[cfg(feature = "local-stt")]
fn resolve_model_path(ai: &ene_ai::AiConfig) -> std::path::PathBuf {
    if let Some(path) = ai
        .stt
        .model_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    if !ai.stt.model.trim().is_empty() {
        return std::path::PathBuf::from(ai.stt.model.trim());
    }
    ene_config::models_dir().join("gguf").join("whisper.gguf")
}

/// Resolve the optional language hint from configuration.
#[cfg(feature = "local-stt")]
fn resolve_language(ai: &ene_ai::AiConfig) -> Option<String> {
    (!ai.stt.language.trim().is_empty()).then(|| ai.stt.language.clone())
}

/// Compute windowed-sinc FIR low-pass coefficients (M15).
///
/// The cutoff is the target Nyquist (8 kHz for a 16 kHz target) so that
/// out-of-band content is attenuated before decimation, preventing aliasing.
/// A Hamming window tapers the impulse response and the taps are normalized
/// for unity DC gain.
#[cfg(feature = "local-stt")]
fn low_pass_coefficients(source_rate: u32) -> Vec<f32> {
    let cutoff = f64::from(WHISPER_SAMPLE_RATE) / 2.0;
    let fc_norm = cutoff / f64::from(source_rate);
    let mid = (FIR_TAPS - 1) as f64 / 2.0;
    let mut coeffs = Vec::with_capacity(FIR_TAPS);
    let mut sum = 0.0_f64;
    for i in 0..FIR_TAPS {
        let x = i as f64 - mid;
        let sinc = if x.abs() < 1e-9 {
            2.0 * fc_norm
        } else {
            (2.0 * std::f64::consts::PI * fc_norm * x).sin() / (std::f64::consts::PI * x)
        };
        let window =
            0.54 - 0.46 * (2.0 * std::f64::consts::PI * i as f64 / (FIR_TAPS - 1) as f64).cos();
        let c = sinc * window;
        coeffs.push(c as f32);
        sum += c;
    }
    if sum.abs() > 1e-9 {
        let norm = sum as f32;
        for c in &mut coeffs {
            *c /= norm;
        }
    }
    coeffs
}

/// Apply an FIR filter to `pcm` (zero-padded at the start).
#[cfg(feature = "local-stt")]
fn apply_fir(pcm: &[f32], coeffs: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(pcm.len());
    for i in 0..pcm.len() {
        let mut acc = 0.0_f32;
        for (k, &c) in coeffs.iter().enumerate() {
            if i >= k
                && let Some(&sample) = pcm.get(i - k)
            {
                acc += c * sample;
            }
        }
        out.push(acc);
    }
    out
}

/// Linear-interpolation resampler to 16 kHz mono.
///
/// whisper.cpp requires a fixed 16 kHz sample rate; microphone capture may
/// arrive at 44.1 kHz / 48 kHz, so we resample defensively before inference.
/// When downsampling, a windowed-sinc FIR low-pass filter is applied first to
/// suppress aliasing (M15).
#[cfg(feature = "local-stt")]
fn resample_to_whisper(pcm: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == WHISPER_SAMPLE_RATE || pcm.is_empty() || sample_rate == 0 {
        return pcm.to_vec();
    }
    // Anti-aliasing: low-pass before decimation when downsampling.
    let filtered = if sample_rate > WHISPER_SAMPLE_RATE {
        let coeffs = low_pass_coefficients(sample_rate);
        apply_fir(pcm, &coeffs)
    } else {
        pcm.to_vec()
    };
    let ratio = f64::from(WHISPER_SAMPLE_RATE) / f64::from(sample_rate);
    let out_len = ((filtered.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64) / ratio;
        let idx = (src_pos.floor() as usize).min(filtered.len().saturating_sub(1));
        let next_idx = idx.saturating_add(1).min(filtered.len().saturating_sub(1));
        let frac = (src_pos - (idx as f64)) as f32;
        let a = filtered.get(idx).copied().unwrap_or(0.0);
        let b = filtered.get(next_idx).copied().unwrap_or(0.0);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Local whisper.cpp speech-to-text provider.
#[cfg(feature = "local-stt")]
pub struct LocalSttProvider {
    ctx: std::sync::Arc<whisper_rs::WhisperContext>,
    /// Cached inference state reused across `transcribe` calls (M14).
    state: parking_lot::Mutex<Option<whisper_rs::WhisperState>>,
    language: Option<String>,
}

#[cfg(feature = "local-stt")]
impl LocalSttProvider {
    /// Load the whisper GGUF model at `model_path`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Init`] when the model file is missing or
    /// whisper.cpp fails to initialize a context.
    pub fn open(
        model_path: &std::path::Path,
        language: Option<String>,
    ) -> Result<Self, AudioProviderError> {
        if !model_path.is_file() {
            return Err(AudioProviderError::Init(format!(
                "whisper GGUF model not found at {}",
                model_path.display()
            )));
        }
        let params = whisper_rs::WhisperContextParameters::default();
        let ctx = whisper_rs::WhisperContext::new_with_params(model_path, params)
            .map_err(|e| AudioProviderError::Init(format!("whisper context init failed: {e}")))?;
        tracing::info!(
            component = "LocalStt",
            path = %model_path.display(),
            "loaded whisper.cpp model"
        );
        Ok(Self {
            ctx: std::sync::Arc::new(ctx),
            state: parking_lot::Mutex::new(None),
            language,
        })
    }

    /// Run whisper inference on already-resampled 16 kHz PCM (blocking),
    /// reusing the cached [`whisper_rs::WhisperState`].
    fn run_inference(
        state: &mut whisper_rs::WhisperState,
        language: Option<&str>,
        pcm: &[f32],
    ) -> Result<SttResult, AudioProviderError> {
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_language(language);

        state
            .full(params, pcm)
            .map_err(|e| AudioProviderError::Provider(format!("whisper inference failed: {e}")))?;

        let mut text = String::new();
        for segment in state.as_iter() {
            if let Ok(part) = segment.to_str_lossy() {
                text.push_str(&part);
            }
        }

        let duration_secs = (pcm.len() as f32) / (WHISPER_SAMPLE_RATE as f32);
        Ok(SttResult {
            text: text.trim().to_string(),
            language: language.map(str::to_string),
            duration_secs,
        })
    }
}

#[cfg(feature = "local-stt")]
#[async_trait]
impl SttProvider for LocalSttProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    async fn transcribe(
        &self,
        pcm: &[f32],
        sample_rate: u32,
    ) -> Result<SttResult, AudioProviderError> {
        let audio = resample_to_whisper(pcm, sample_rate);
        let ctx = std::sync::Arc::clone(&self.ctx);
        let language = self.language.clone();
        // Take the cached state out so it can move into the blocking task; it
        // is returned afterwards for reuse (M14).
        let cached = self.state.lock().take();
        let (state, result) = tokio::task::spawn_blocking(move || {
            let mut state = match cached {
                Some(state) => state,
                None => match ctx.create_state() {
                    Ok(state) => state,
                    Err(e) => {
                        return (
                            None,
                            Err(AudioProviderError::Init(format!(
                                "whisper state init failed: {e}"
                            ))),
                        );
                    }
                },
            };
            let result = Self::run_inference(&mut state, language.as_deref(), &audio);
            (Some(state), result)
        })
        .await
        .map_err(|e| AudioProviderError::Provider(format!("whisper task join error: {e}")))?;
        *self.state.lock() = state;
        result
    }
}

/// Factory for the local whisper.cpp STT provider.
pub struct LocalSttProviderFactory;

#[cfg(feature = "local-stt")]
impl LocalSttProviderFactory {
    fn build(config: &ene_config::EneConfig) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        let ai = config
            .get_section::<ene_ai::AiConfig>()
            .map_err(|e| AudioProviderError::Init(format!("failed to parse AI config: {e}")))?;
        let path = resolve_model_path(&ai);
        let language = resolve_language(&ai);
        let provider = LocalSttProvider::open(&path, language)?;
        Ok(Box::new(provider))
    }
}

#[cfg(not(feature = "local-stt"))]
impl LocalSttProviderFactory {
    fn build(_config: &ene_config::EneConfig) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        Err(AudioProviderError::Init(
            "local STT requested but ene-ai was built without the `local-stt` feature".to_string(),
        ))
    }
}

impl SttProviderFactory for LocalSttProviderFactory {
    fn provider_name(&self) -> &str {
        PROVIDER_NAME
    }

    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        Self::build(config)
    }
}

/// Register the local whisper STT factory at startup.
///
/// Registered unconditionally; the factory fails fast at `create_provider`
/// time when the `local-stt` feature is disabled.
#[ctor::ctor(unsafe)]
fn register_local_stt() {
    AudioProviderRegistry::register_stt(std::sync::Arc::new(LocalSttProviderFactory));
}

#[cfg(all(test, feature = "local-stt"))]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_at_16khz() {
        let pcm = vec![0.1, -0.2, 0.3, 0.0];
        let out = resample_to_whisper(&pcm, WHISPER_SAMPLE_RATE);
        assert_eq!(out, pcm);
    }

    #[test]
    fn resample_correct_length_at_48khz() {
        let pcm = vec![0.0; 4800];
        let out = resample_to_whisper(&pcm, 48_000);
        // 4800 samples at 48 kHz -> 1600 samples at 16 kHz.
        assert_eq!(out.len(), 1600);
    }

    #[test]
    fn resample_empty_input() {
        let out = resample_to_whisper(&[], 48_000);
        assert!(out.is_empty());
    }

    #[test]
    fn resample_zero_rate_is_identity() {
        let pcm = vec![0.5, 0.25];
        let out = resample_to_whisper(&pcm, 0);
        assert_eq!(out, pcm);
    }

    #[test]
    fn fir_preserves_dc() {
        // A constant (DC) signal should pass a unity-DC-gain low-pass roughly
        // unchanged once the filter settles.
        let coeffs = low_pass_coefficients(48_000);
        let dc = vec![1.0_f32; 64];
        let out = apply_fir(&dc, &coeffs);
        let tail = out.last().copied().unwrap_or(0.0);
        assert!((tail - 1.0).abs() < 0.05, "DC tail = {tail}");
    }
}
