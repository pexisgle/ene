//! Local speech-to-text provider backed by `whisper.cpp` (`whisper-rs`).
//!
//! The heavy native dependency is gated behind the `local-stt` cargo feature.
//! When the feature is disabled, [`LocalSttProviderFactory`] still registers
//! but fails fast with [`AudioProviderError::Init`] so the crate (and the
//! workspace) keeps compiling without the whisper.cpp toolchain.
//!
//! # Migration to `ene_infer::LocalModel`
//!
//! This provider used to hold a `parking_lot::Mutex<Option<WhisperState>>`
//! and `.lock().take()` the cached state out around a `tokio::task::spawn_blocking`
//! call, putting it back afterward (M14). That pattern had three independent
//! bugs: two concurrent `transcribe` calls could both observe `None` and each
//! allocate a fresh multi-hundred-megabyte `WhisperState` (one silently
//! clobbering the other on put-back), a cancelled or panicking task never
//! reached the put-back line and lost the cached state permanently, and
//! nothing actually serialized whisper inference — the mutex only protected
//! the `Option`, not the inference call itself.
//!
//! [`WhisperModel`] deletes all of that: `state` is now a plain field owned
//! exclusively by the single worker thread [`ene_infer::EngineHandle`] spawns
//! for it. There is no `Arc`, no `Mutex`, and no take/put-back — a cancelled
//! or panicking job simply leaves the field where it is (or the whole model
//! is rebuilt from the factory on panic).
#![allow(
    clippy::arithmetic_side_effects,
    reason = "FIR filter and resampler use bounded counter arithmetic over PCM buffers"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "resampler indexes into bounded PCM buffers with clamped positions"
)]

#[cfg(feature = "local-stt")]
use std::sync::Arc;
#[cfg(feature = "local-stt")]
use std::time::Duration;

use ene_ai::{AudioProviderError, AudioProviderRegistry, SttProvider, SttProviderFactory};
#[cfg(feature = "local-stt")]
use ene_ai::{Capability, CapabilitySet, EngineDescriptor, LocalSttEngine, ResourceClass};
#[cfg(feature = "local-stt")]
use ene_infer::{EngineConfig, EngineHandle, JobContext, LocalModel};

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

/// Errors produced by [`WhisperModel`] itself, as distinct from the
/// framework-level [`ene_infer::EngineError`] conditions (busy, timeout,
/// cancelled, engine down) that [`ene_ai::LocalSttEngine`] maps to
/// [`AudioProviderError`] independently of this type.
#[cfg(feature = "local-stt")]
#[derive(Debug, thiserror::Error)]
pub enum WhisperError {
    /// `whisper.cpp` failed to allocate a fresh inference state, either at
    /// startup or when rebuilding the model after a panic.
    #[error("whisper state init failed: {0}")]
    StateInit(String),
    /// The `state.full()` inference call itself failed.
    #[error("whisper inference failed: {0}")]
    Inference(String),
    /// [`ene_infer::JobContext::should_stop`] had already fired before
    /// inference started (the job was cancelled, or its deadline elapsed,
    /// while queued behind another job).
    #[error("job stopped before whisper inference started")]
    StoppedEarly,
}

/// Number of whisper.cpp transcription jobs allowed to queue behind the one
/// currently executing. One in flight plus one queued matches
/// [`ene_ai::ConcurrencyHint::default`]'s conservative sizing for a
/// single-worker local model; a third concurrent caller gets
/// [`AudioProviderError::Busy`] immediately rather than piling up latency.
#[cfg(feature = "local-stt")]
const STT_QUEUE_DEPTH: usize = 2;

/// Generous upper bound on a single whisper.cpp transcription call, comfortably
/// longer than any realistic utterance on CPU with a base/small GGUF model.
///
/// This mostly matters for a job that is still *queued* when its deadline
/// elapses (see [`WhisperModel::run`]'s upfront `should_stop` check) — once
/// `state.full()` actually starts, whisper.cpp gives this crate no callback
/// to interrupt it mid-call (see [`STT_STALL_TIMEOUT`] below), so this bound
/// cannot preempt an in-progress inference. It is kept generous rather than
/// tight for exactly that reason: a tight timeout here would not actually
/// speed up cancellation, it would only make a legitimately slow-but-alive
/// transcription more likely to be reported as [`AudioProviderError::Timeout`]
/// after the fact.
#[cfg(feature = "local-stt")]
const STT_JOB_TIMEOUT: Duration = Duration::from_mins(1);

/// Documents why this engine does not set
/// [`ene_infer::EngineConfig::stall_timeout`] (kept at the crate default of
/// `None`): `whisper_rs::WhisperState::full` is a single blocking FFI call
/// with no natural interruption point this code hooks into, so
/// [`ene_infer::JobContext::tick`] can only be called immediately before and
/// after it, never *during* it. A `stall_timeout` measures the gap since the
/// last `tick`, so any value shorter than the slowest realistic transcription
/// would misidentify a merely slow (but healthy) job as a wedged worker and
/// permanently disable the engine (see
/// [`ene_infer::EngineConfig::stall_timeout`]'s docs). `whisper_rs` does
/// expose `FullParams::set_abort_callback_safe`/`set_progress_callback_safe`,
/// which could give real mid-call cancellation and a genuine tick source —
/// left as a follow-up, not attempted here, since it requires either an
/// `unsafe` pointer-lifetime workaround or a second polling thread per job to
/// satisfy the callback's `'static` bound, which is more machinery than this
/// migration's scope (replacing the take/put-back mutex pattern) calls for.
#[cfg(feature = "local-stt")]
const STT_STALL_TIMEOUT: Option<Duration> = None;

/// The exclusively-owned whisper.cpp inference model.
///
/// Owned by exactly one [`ene_infer::EngineHandle`] worker thread for its
/// entire lifetime — see this module's migration doc comment for what that
/// replaces. `state` is reused across jobs purely to avoid reallocating it
/// per request (each `full()` call reprocesses its input from scratch;
/// whisper.cpp does not carry decode state between calls), so
/// [`ene_infer::LocalModel::reset`] has nothing to do and is not overridden.
#[cfg(feature = "local-stt")]
pub struct WhisperModel {
    state: whisper_rs::WhisperState,
    language: Option<String>,
}

#[cfg(feature = "local-stt")]
impl WhisperModel {
    /// Builds a fresh model from an already-loaded [`whisper_rs::WhisperContext`].
    ///
    /// Called once by [`open`] and again by [`ene_infer::EngineHandle`]
    /// every time a panicked `run`/`reset` call forces a rebuild — `ctx` is
    /// cheap to reuse (it wraps its own `Arc`-backed native handle) but a
    /// fresh [`whisper_rs::WhisperState`] must be allocated each time.
    fn new(
        ctx: &whisper_rs::WhisperContext,
        language: Option<String>,
    ) -> Result<Self, WhisperError> {
        let state = ctx
            .create_state()
            .map_err(|e| WhisperError::StateInit(e.to_string()))?;
        Ok(Self { state, language })
    }
}

#[cfg(feature = "local-stt")]
impl LocalModel for WhisperModel {
    type Request = ene_ai::SttTranscribeRequest;
    type Response = ene_ai::SttResult;
    type Error = WhisperError;

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
    )]
    fn engine_name(&self) -> &str {
        "whisper"
    }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        // The only natural interruption point available: once `full()` below
        // starts, it runs to completion (see `STT_STALL_TIMEOUT`'s docs).
        // This still lets a job that went stale while queued behind another
        // one bail out instead of paying for a transcription nobody wants.
        if ctx.should_stop().is_some() {
            return Err(WhisperError::StoppedEarly);
        }
        ctx.tick();

        let audio = resample_to_whisper(&req.pcm, req.sample_rate);
        let language = self.language.clone();

        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_language(language.as_deref());

        self.state
            .full(params, &audio)
            .map_err(|e| WhisperError::Inference(e.to_string()))?;
        ctx.tick();

        let mut text = String::new();
        for segment in self.state.as_iter() {
            if let Ok(part) = segment.to_str_lossy() {
                text.push_str(&part);
            }
        }

        let duration_secs = (audio.len() as f32) / (WHISPER_SAMPLE_RATE as f32);
        Ok(ene_ai::SttResult {
            text: text.trim().to_string(),
            language,
            duration_secs,
        })
    }
}

/// Load the whisper GGUF model at `model_path` and spawn its dedicated
/// [`ene_infer::EngineHandle`] worker thread.
///
/// # Errors
///
/// Returns [`AudioProviderError::Init`] when the model file is missing,
/// whisper.cpp fails to initialize a context, or the first
/// [`whisper_rs::WhisperState`] fails to allocate ([`EngineHandle::try_spawn`]
/// builds it synchronously here, so this failure is not deferred to the
/// first [`SttProvider::transcribe`] call). Once the handle is returned,
/// per-job failures surface through [`SttProvider::transcribe`] instead.
#[cfg(feature = "local-stt")]
fn open(
    model_path: &std::path::Path,
    language: Option<String>,
) -> Result<LocalSttEngine<WhisperModel>, AudioProviderError> {
    if !model_path.is_file() {
        return Err(AudioProviderError::Init(format!(
            "whisper GGUF model not found at {}",
            model_path.display()
        )));
    }
    let params = whisper_rs::WhisperContextParameters::default();
    let ctx = whisper_rs::WhisperContext::new_with_params(model_path, params)
        .map_err(|e| AudioProviderError::Init(format!("whisper context init failed: {e}")))?;
    let ctx = Arc::new(ctx);
    tracing::info!(
        component = "LocalStt",
        path = %model_path.display(),
        "loaded whisper.cpp model"
    );

    let factory = {
        let ctx = Arc::clone(&ctx);
        move || WhisperModel::new(&ctx, language.clone())
    };
    let mut cfg = EngineConfig::new(STT_QUEUE_DEPTH, STT_JOB_TIMEOUT);
    if let Some(stall) = STT_STALL_TIMEOUT {
        cfg = cfg.with_stall_timeout(stall);
    }
    // `try_spawn` (not `spawn`) builds the first `WhisperModel` synchronously
    // here rather than deferring that first `factory()` call to the worker
    // thread, so a `WhisperError::StateInit` failure surfaces as this
    // function's own `AudioProviderError::Init` instead of a deferred
    // `EngineDown` on the first `transcribe` call.
    let handle = EngineHandle::try_spawn(factory, cfg)
        .map_err(|e| AudioProviderError::Init(e.to_string()))?;

    let descriptor = EngineDescriptor::new(
        PROVIDER_NAME,
        CapabilitySet::empty().with(Capability::Stt),
        // whisper.cpp runs CPU-only in this workspace today (no GPU feature
        // is enabled for `whisper-rs`, so `WhisperContextParameters::default()`
        // leaves `use_gpu` false). `ResourceClass::Cpu` is shared by every
        // CPU-bound local engine (see its type docs in `ene_ai`) — whisper
        // and Kokoro (`local_tts.rs`) may run concurrently up to
        // `ResourceRegistry`'s shared CPU budget, not because either engine
        // picked a distinguishing number.
        ResourceClass::Cpu,
    );
    Ok(LocalSttEngine::new(handle, descriptor))
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
        let engine = open(&path, language)?;
        Ok(Box::new(engine))
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

/// Runs `ene_infer::conformance::run_all` against a test-only stand-in for
/// [`WhisperModel`] (not gated behind `local-stt`: it needs neither
/// `whisper-rs` nor a model file, only the same [`ene_infer::LocalModel`]
/// shape).
///
/// [`ConformanceRequest`]/[`ConformanceResponse`] cannot be implemented on
/// [`WhisperModel`]'s real `ene_ai::SttTranscribeRequest`/`ene_ai::SttResult`
/// types directly — neither the trait nor the type is local to this crate,
/// so that would violate the orphan rule, and even if it didn't, "run for
/// approximately this long" has no meaningful encoding as raw PCM samples.
/// [`ScriptedSttModel`] validates the part that *is* generic: that this
/// engine's [`ene_infer::EngineHandle`]/[`ene_infer::EngineConfig`]/factory
/// wiring gets queueing, cancellation, panic recovery, and post-cancel
/// `reset` right. It deliberately does *not* reproduce
/// [`WhisperModel::run`]'s one-check-at-the-start limitation (see that
/// method's doc comment) — a well-behaved [`ene_infer::LocalModel`] ticks
/// throughout its work, and this battery is what confirms the framework
/// wiring around one behaves correctly, independent of whisper.cpp's own
/// inability to honor that mid-call.
#[cfg(test)]
mod conformance_tests {
    use std::time::{Duration, Instant};

    use ene_infer::conformance::{ConformanceRequest, ConformanceResponse};
    use ene_infer::{JobContext, LocalModel};

    #[derive(Debug, Clone, Default)]
    struct ScriptedSttRequest {
        run_for: Duration,
        then_panic: bool,
    }

    impl ConformanceRequest for ScriptedSttRequest {
        fn scripted(run_for: Duration, then_panic: bool) -> Self {
            Self {
                run_for,
                then_panic,
            }
        }
    }

    #[derive(Debug)]
    struct ScriptedSttResponse {
        resets_seen: usize,
    }

    impl ConformanceResponse for ScriptedSttResponse {
        fn resets_seen(&self) -> usize {
            self.resets_seen
        }
    }

    #[derive(Debug, thiserror::Error)]
    #[error("scripted whisper stand-in stopped cooperatively")]
    struct ScriptedSttError;

    #[derive(Debug, Default)]
    struct ScriptedSttModel {
        resets_seen: usize,
    }

    impl LocalModel for ScriptedSttModel {
        type Request = ScriptedSttRequest;
        type Response = ScriptedSttResponse;
        type Error = ScriptedSttError;

        #[expect(
            clippy::unnecessary_literal_bound,
            reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
        )]
        fn engine_name(&self) -> &str {
            "scripted-whisper"
        }

        fn run(
            &mut self,
            req: Self::Request,
            ctx: &JobContext,
        ) -> Result<Self::Response, Self::Error> {
            assert!(!req.then_panic, "scripted panic for conformance testing");
            let start = Instant::now();
            loop {
                if ctx.should_stop().is_some() {
                    return Err(ScriptedSttError);
                }
                ctx.tick();
                if start.elapsed() >= req.run_for {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(ScriptedSttResponse {
                resets_seen: self.resets_seen,
            })
        }

        fn reset(&mut self) {
            self.resets_seen += 1;
        }
    }

    #[tokio::test]
    async fn whisper_engine_wiring_passes_conformance_battery() {
        ene_infer::conformance::run_all(ScriptedSttModel::default).await;
    }
}
