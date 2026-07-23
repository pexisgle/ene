//! Local speech-to-text provider backed by `whisper.cpp` (`whisper-rs`).
//!
//! The heavy native dependency is gated behind the `local-stt` cargo feature.
//! When the feature is disabled, [`LocalSttProviderFactory`] still registers
//! but fails fast with [`AudioProviderError::Init`] so the crate (and the
//! workspace) keeps compiling without the whisper.cpp toolchain.

#[cfg(feature = "local-stt")]
use crate::audio::SttResult;
use crate::audio::{AudioProviderError, AudioProviderRegistry, SttProvider, SttProviderFactory};
#[cfg(feature = "local-stt")]
use async_trait::async_trait;

/// Provider name used in `ai.stt.provider` configuration.
pub const PROVIDER_NAME: &str = "whisper";

/// whisper.cpp operates on 16 kHz mono PCM.
#[cfg(feature = "local-stt")]
const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// Resolve the whisper GGUF model path from configuration / environment.
///
/// Precedence: `ENE_AI__STT__MODEL_PATH` env, then `SttConfig::model` when it
/// looks like a filesystem path, then a default cache location.
#[cfg(feature = "local-stt")]
fn resolve_model_path(config: &ene_config::EneConfig) -> std::path::PathBuf {
    if let Ok(path) = std::env::var("ENE_AI__STT__MODEL_PATH")
        && !path.trim().is_empty()
    {
        return std::path::PathBuf::from(path.trim());
    }
    if let Ok(ai) = config.get_section::<crate::config::AiConfig>()
        && !ai.stt.model.trim().is_empty()
    {
        return std::path::PathBuf::from(ai.stt.model.trim());
    }
    crate::gguf::gguf_cache_dir().join("whisper.gguf")
}

/// Resolve the optional language hint from configuration.
#[cfg(feature = "local-stt")]
fn resolve_language(config: &ene_config::EneConfig) -> Option<String> {
    config
        .get_section::<crate::config::AiConfig>()
        .ok()
        .and_then(|ai| (!ai.stt.language.trim().is_empty()).then(|| ai.stt.language.clone()))
}

/// Linear-interpolation resampler to 16 kHz mono.
///
/// whisper.cpp requires a fixed 16 kHz sample rate; microphone capture may
/// arrive at 44.1 kHz / 48 kHz, so we resample defensively before inference.
#[cfg(feature = "local-stt")]
fn resample_to_whisper(pcm: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == WHISPER_SAMPLE_RATE || pcm.is_empty() || sample_rate == 0 {
        return pcm.to_vec();
    }
    let ratio = f64::from(WHISPER_SAMPLE_RATE) / f64::from(sample_rate);
    let out_len = ((pcm.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64) / ratio;
        let idx = (src_pos.floor() as usize).min(pcm.len().saturating_sub(1));
        let next_idx = idx.saturating_add(1).min(pcm.len().saturating_sub(1));
        let frac = (src_pos - (idx as f64)) as f32;
        let a = pcm.get(idx).copied().unwrap_or(0.0);
        let b = pcm.get(next_idx).copied().unwrap_or(0.0);
        out.push(a + (b - a) * frac);
    }
    out
}

/// Local whisper.cpp speech-to-text provider.
#[cfg(feature = "local-stt")]
pub struct LocalSttProvider {
    ctx: std::sync::Arc<whisper_rs::WhisperContext>,
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
            language,
        })
    }

    /// Run whisper inference on already-resampled 16 kHz PCM (blocking).
    fn run_inference(
        ctx: &whisper_rs::WhisperContext,
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

        let mut state = ctx
            .create_state()
            .map_err(|e| AudioProviderError::Init(format!("whisper state init failed: {e}")))?;
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
        // whisper.cpp inference is CPU-bound and blocking; run it off the
        // async runtime so concurrent turns are not starved.
        tokio::task::spawn_blocking(move || Self::run_inference(&ctx, language.as_deref(), &audio))
            .await
            .map_err(|e| AudioProviderError::Provider(format!("whisper task join error: {e}")))?
    }
}

/// Factory for the local whisper.cpp STT provider.
pub struct LocalSttProviderFactory;

#[cfg(feature = "local-stt")]
impl LocalSttProviderFactory {
    fn build(config: &ene_config::EneConfig) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        let path = resolve_model_path(config);
        let language = resolve_language(config);
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
