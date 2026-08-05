//! `local-tts`-gated Kokoro ONNX provider: the ONNX session, voice
//! embeddings, and the [`ene_ai::TtsProvider`] wiring.
//!
//! # Why `KokoroModel` owns its session exclusively
//!
//! The `ort::session::Session` is a plain field owned exclusively by the
//! single worker thread [`ene_infer::EngineHandle`] spawns for it, with no
//! `Arc`/`Mutex` and no `spawn_blocking` anywhere in this file: a session
//! shared behind a mutex and invoked from `spawn_blocking` is exactly the
//! bug class [`ene_infer::LocalModel`] exists to remove.
//! [`ene_ai::LocalTtsEngine`] preserves the existing chunked-delivery shape
//! exactly: one batch inference produces a full PCM buffer, which is then
//! sliced into [`ene_ai::DEFAULT_CHUNK_SAMPLES`]-sized chunks pushed through
//! an `mpsc` channel — this is not, and must not become, true streaming
//! synthesis.
use ene_ai::{
    AudioProviderError, Capability, CapabilitySet, EngineDescriptor, LocalTtsEngine, ResourceClass,
};
use ene_infer::{EngineConfig, EngineHandle, JobContext, LocalModel};
use std::time::Duration;

use super::download::VOICE_DIM;

/// Kokoro ONNX emits 24 kHz mono PCM.
const KOKORO_SAMPLE_RATE: u32 = 24_000;

/// Known Kokoro-82M (v1.0) voice names mapped to their stacked index in
/// `voices.bin`. `voices.bin` is a flat little-endian `f32` array of
/// `VOICE_DIM`-float vectors concatenated in this order.
const KOKORO_VOICES: &[(&str, usize)] = &[
    ("af_alloy", 0),
    ("af_aoede", 1),
    ("af_bella", 2),
    ("af_heart", 3),
    ("af_jessica", 4),
    ("af_kore", 5),
    ("af_nicole", 6),
    ("af_nova", 7),
    ("af_river", 8),
    ("af_sarah", 9),
    ("af_sky", 10),
    ("am_adam", 11),
    ("am_echo", 12),
    ("am_eric", 13),
    ("am_fenrir", 14),
    ("am_liam", 15),
    ("am_michael", 16),
    ("am_onyx", 17),
    ("am_puck", 18),
    ("am_santa", 19),
    ("bf_alice", 20),
    ("bf_emma", 21),
    ("bf_isabella", 22),
    ("bf_lily", 23),
    ("bm_daniel", 24),
    ("bm_fable", 25),
    ("bm_george", 26),
    ("bm_lewis", 27),
    ("ef_dora", 28),
    ("em_alex", 29),
    ("ff_siwis", 30),
    ("hf_alpha", 31),
    ("hf_beta", 32),
    ("hm_omega", 33),
    ("hm_psi", 34),
    ("if_sara", 35),
    ("im_nicola", 36),
    ("jf_alpha", 37),
    ("jf_gongitsune", 38),
    ("jf_nezumi", 39),
    ("jf_tebukuro", 40),
    ("jm_kumo", 41),
    ("pf_dora", 42),
    ("pm_alex", 43),
    ("pm_santa", 44),
    ("zf_xiaobei", 45),
    ("zf_xiaoni", 46),
    ("zf_xiaoxiao", 47),
    ("zf_xiaoyi", 48),
    ("zm_yunjian", 49),
    ("zm_yunxi", 50),
    ("zm_yunxia", 51),
    ("zm_yunyang", 52),
];

/// Errors produced by [`KokoroModel`] itself, as distinct from the
/// framework-level [`ene_infer::EngineError`] conditions (busy, timeout,
/// cancelled, engine down) that [`ene_ai::LocalTtsEngine`] maps to
/// [`AudioProviderError`] independently of this type.
#[derive(Debug, thiserror::Error)]
pub enum KokoroError {
    /// The model/voice file is missing, the requested voice is unknown, or
    /// ONNX Runtime failed to build a session — surfaced both by [`open`]'s
    /// eager, fail-fast first build and by any later rebuild after a panic.
    #[error("Kokoro model init failed: {0}")]
    Init(String),
    /// Building one of the ONNX Runtime input tensors failed.
    #[error("ONNX input build failed: {0}")]
    Input(String),
    /// The `session.run()` inference call itself failed, or its output was
    /// missing/malformed.
    #[error("ONNX inference failed: {0}")]
    Inference(String),
    /// [`ene_infer::JobContext::should_stop`] had already fired before
    /// inference started (the job was cancelled, or its deadline elapsed,
    /// while queued behind another job).
    #[error("job stopped before Kokoro inference started")]
    StoppedEarly,
}

/// Number of Kokoro synthesis jobs allowed to queue behind the one currently
/// executing. See `local_stt.rs::STT_QUEUE_DEPTH` for the identical reasoning.
const TTS_QUEUE_DEPTH: usize = 2;

/// Generous upper bound on a single Kokoro synthesis call, comfortably longer
/// than any realistic utterance of text on CPU.
///
/// As with whisper (`local_stt.rs::STT_JOB_TIMEOUT`), this only reliably
/// preempts a job that is still queued when its deadline elapses:
/// `ort::session::Session::run` is a single opaque call this crate cannot
/// interrupt mid-flight once it has started (see [`TTS_STALL_TIMEOUT`]).
const TTS_JOB_TIMEOUT: Duration = Duration::from_mins(1);

/// Kept at the crate default (`None`) for the same reason as whisper's
/// `local_stt.rs::STT_STALL_TIMEOUT`: `ort::session::Session::run` is a
/// single blocking FFI call with no callback this code hooks into for
/// progress, so [`ene_infer::JobContext::tick`] can only run immediately
/// before and after it. Any `stall_timeout` shorter than the slowest
/// realistic synthesis call would misidentify a merely slow (but healthy)
/// job as a wedged worker and permanently disable the engine.
const TTS_STALL_TIMEOUT: Option<Duration> = None;

/// The exclusively-owned Kokoro ONNX inference model.
///
/// Owned by exactly one [`ene_infer::EngineHandle`] worker thread for its
/// entire lifetime — see the module-level doc comment for why exclusive
/// ownership matters. Kokoro's
/// `session.run()` is a stateless batch call (no recurrent/decoder state
/// carried between requests), so [`ene_infer::LocalModel::reset`] has
/// nothing to do and is not overridden.
pub struct KokoroModel {
    session: ort::session::Session,
    voice: Vec<f32>,
    speed: f32,
    language: Option<String>,
}

impl KokoroModel {
    /// Builds a fresh model, loading the ONNX session and voice embedding
    /// from disk.
    ///
    /// Called once by [`open`] and again by [`ene_infer::EngineHandle`] every
    /// time a panicked `run`/`reset` call forces a rebuild.
    ///
    /// # Errors
    ///
    /// Returns [`KokoroError::Init`] when the model or voice file is missing,
    /// the requested voice is unknown, or ONNX Runtime fails to build a
    /// session.
    fn new(
        model_path: &std::path::Path,
        voices_path: &std::path::Path,
        voice_name: &str,
        speed: f32,
        language: Option<String>,
    ) -> Result<Self, KokoroError> {
        if !model_path.is_file() {
            return Err(KokoroError::Init(format!(
                "Kokoro ONNX model not found at {}; fetch it before opening the TTS provider \
                 (see Settings > Voice, or let bootstrap prefetch it automatically)",
                model_path.display()
            )));
        }
        if !voices_path.is_file() {
            return Err(KokoroError::Init(format!(
                "Kokoro voices.bin not found at {}; fetch it before opening the TTS provider \
                 (see Settings > Voice, or let bootstrap prefetch it automatically)",
                voices_path.display()
            )));
        }
        let session = ort::session::Session::builder()
            .map_err(|e| KokoroError::Init(format!("ONNX session builder failed: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| KokoroError::Init(format!("ONNX session load failed: {e}")))?;

        let voice = load_voice_embedding(voices_path, voice_name)
            .map_err(|e| KokoroError::Init(e.to_string()))?;

        tracing::info!(
            component = "LocalTts",
            model = %model_path.display(),
            voices = %voices_path.display(),
            voice = voice_name,
            "loaded Kokoro ONNX model"
        );
        Ok(Self {
            session,
            voice,
            speed,
            language,
        })
    }

    /// Run Kokoro inference over pre-tokenized phoneme ids and return the full
    /// PCM buffer.
    fn run_inference(&mut self, tokens: &[i64]) -> Result<Vec<f32>, KokoroError> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let seq_len = tokens.len();

        let style = self.voice.clone();
        let token_buf = tokens.to_vec();
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => ort::value::Tensor::from_array((
                    [1_i64, seq_len as i64],
                    token_buf,
                ))
                .map_err(|e| KokoroError::Input(format!("input_ids: {e}")))?,
                "style" => ort::value::Tensor::from_array((
                    [1_i64, style.len() as i64],
                    style,
                ))
                .map_err(|e| KokoroError::Input(format!("style: {e}")))?,
                "speed" => ort::value::Tensor::from_array(([1_i64], vec![self.speed]))
                .map_err(|e| KokoroError::Input(format!("speed: {e}")))?,
            ])
            .map_err(|e| KokoroError::Inference(format!("ONNX inference failed: {e}")))?;

        let output = outputs
            .get("output")
            .ok_or_else(|| KokoroError::Inference("no `output` tensor produced".to_string()))?;
        let (_shape, data) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| KokoroError::Inference(format!("output extraction failed: {e}")))?;
        Ok(data.to_vec())
    }
}

impl LocalModel for KokoroModel {
    type Request = ene_ai::TtsSynthesisRequest;
    type Response = ene_ai::TtsSynthesisResponse;
    type Error = KokoroError;

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "must match LocalModel::engine_name's trait signature, which ties the return type to &self's lifetime"
    )]
    fn engine_name(&self) -> &str {
        "kokoro"
    }

    fn run(&mut self, req: Self::Request, ctx: &JobContext) -> Result<Self::Response, Self::Error> {
        // See `local_stt.rs::WhisperModel::run` for why this is the only
        // interruption point available: once `session.run()` starts below,
        // it runs to completion (see `TTS_STALL_TIMEOUT`'s docs).
        if ctx.should_stop().is_some() {
            return Err(KokoroError::StoppedEarly);
        }
        ctx.tick();

        let tokens = crate::g2p::text_to_tokens(&req.text, self.language.as_deref());
        // `text_to_tokens` always emits BOS/EOS; anything beyond that is audio.
        if tokens.len() <= 2 {
            return Ok(ene_ai::TtsSynthesisResponse {
                pcm: Vec::new(),
                sample_rate: KOKORO_SAMPLE_RATE,
            });
        }

        let pcm = self.run_inference(&tokens)?;
        ctx.tick();

        Ok(ene_ai::TtsSynthesisResponse {
            pcm,
            sample_rate: KOKORO_SAMPLE_RATE,
        })
    }
}

fn voice_index(name: &str) -> Option<usize> {
    KOKORO_VOICES
        .iter()
        .find(|(voice, _)| *voice == name)
        .map(|(_, idx)| *idx)
}

/// Load a single [`VOICE_DIM`]-dim voice embedding for `voice_name` from
/// `voices.bin`.
///
/// `voices.bin` is a flat little-endian `f32` array of stacked voice vectors
/// (see [`KOKORO_VOICES`]). An empty `voice_name` selects the first voice.
///
/// # Errors
///
/// Returns [`AudioProviderError::Init`] when the file is missing, its byte
/// length is not a multiple of four, it is too small for the requested
/// voice, or `voice_name` is not a known voice.
fn load_voice_embedding(
    voices_path: &std::path::Path,
    voice_name: &str,
) -> Result<Vec<f32>, AudioProviderError> {
    if !voices_path.is_file() {
        return Err(AudioProviderError::Init(format!(
            "Kokoro voices.bin not found at {}",
            voices_path.display()
        )));
    }
    let bytes = std::fs::read(voices_path)?;
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(AudioProviderError::Init(format!(
            "voices.bin length {} is not a multiple of {} bytes",
            bytes.len(),
            std::mem::size_of::<f32>()
        )));
    }
    let floats: Vec<f32> = bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|chunk| {
            let mut le = [0_u8; std::mem::size_of::<f32>()];
            le.copy_from_slice(chunk);
            f32::from_le_bytes(le)
        })
        .collect();

    let index = if voice_name.trim().is_empty() {
        0
    } else {
        voice_index(voice_name).ok_or_else(|| {
            let available: Vec<&str> = KOKORO_VOICES.iter().map(|(name, _)| *name).collect();
            AudioProviderError::Init(format!(
                "unknown Kokoro voice '{voice_name}'; available voices: {}",
                available.join(", ")
            ))
        })?
    };

    let start = index * VOICE_DIM;
    let end = start + VOICE_DIM;
    if floats.len() < end {
        return Err(AudioProviderError::Init(format!(
            "voices.bin too small: {} floats, need at least {end} for voice index {index}",
            floats.len()
        )));
    }
    Ok(floats[start..end].to_vec())
}

/// Load the Kokoro ONNX model and the selected voice embedding, then spawn
/// its dedicated [`ene_infer::EngineHandle`] worker thread.
///
/// ONNX Runtime is initialized exactly once per process using
/// `ort_dylib_path` when provided. The voice named by `voice_name` (e.g.
/// `"af_heart"`) is selected from `voices.bin`; an empty name selects the
/// first voice.
///
/// # Errors
///
/// Returns [`AudioProviderError::Init`] when the model or voice file is
/// missing, the requested voice is unknown, or ONNX Runtime fails to build a
/// session. Once the handle is returned, per-job failures surface through
/// [`ene_ai::TtsProvider::synthesize_stream`] instead.
///
/// This never performs network I/O: it is a synchronous, fail-fast
/// constructor. Callers are responsible for ensuring the model files are
/// already present (see [`super::prefetch_if_configured`], called from the
/// runtime's async bootstrap path before providers are constructed).
///
/// [`EngineHandle::try_spawn`] (not [`EngineHandle::spawn`]) builds the first
/// [`KokoroModel`] synchronously here rather than deferring that first
/// `factory()` call to the worker thread, so a missing model file or a bad
/// ONNX session fails right here with [`AudioProviderError::Init`] instead
/// of being silently deferred to the first
/// [`ene_ai::TtsProvider::synthesize_stream`] call reporting an opaque
/// `EngineDown` — which is when the runtime bootstrap path expects to see
/// this error (and log a clear warning), not on first use.
pub fn open(
    model_path: &std::path::Path,
    voices_path: &std::path::Path,
    voice_name: &str,
    speed: f32,
    language: Option<String>,
    ort_dylib_path: Option<&str>,
) -> Result<LocalTtsEngine<KokoroModel>, AudioProviderError> {
    crate::ort_init::ensure_ort_init(ort_dylib_path)?;

    let model_path = model_path.to_path_buf();
    let voices_path = voices_path.to_path_buf();
    let voice_name = voice_name.to_string();
    let factory = move || {
        KokoroModel::new(
            &model_path,
            &voices_path,
            &voice_name,
            speed,
            language.clone(),
        )
    };
    let mut cfg = EngineConfig::new(TTS_QUEUE_DEPTH, TTS_JOB_TIMEOUT);
    if let Some(stall) = TTS_STALL_TIMEOUT {
        cfg = cfg.with_stall_timeout(stall);
    }
    let handle = EngineHandle::try_spawn(factory, cfg)
        .map_err(|e| AudioProviderError::Init(e.to_string()))?;

    let descriptor = EngineDescriptor::new(
        super::PROVIDER_NAME,
        CapabilitySet::empty().with(Capability::Tts),
        // Kokoro-82M runs CPU-only in this workspace today (the `ort`
        // dependency enables no GPU execution provider feature).
        // `ResourceClass::Cpu` is shared by every CPU-bound local engine
        // (see its type docs in `ene_ai`) — Kokoro and whisper
        // (`local_stt.rs`) may run concurrently up to `ResourceRegistry`'s
        // shared CPU budget, not because either engine picked a
        // distinguishing number.
        ResourceClass::Cpu,
    );
    Ok(LocalTtsEngine::new(handle, descriptor))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "voice embeddings are written and read back as exact f32 bit patterns"
    )]
    #![expect(clippy::expect_used, reason = "unit tests use expect for assertions")]

    use super::*;
    use std::io::Write;

    fn write_voices(path: &std::path::Path, num_voices: usize) {
        let mut bytes = Vec::new();
        for v in 0..num_voices {
            for i in 0..VOICE_DIM {
                // Encode (voice, dim) so tests can assert the right slice.
                let value = (v * VOICE_DIM + i) as f32;
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let mut file = std::fs::File::create(path).expect("create voices file");
        file.write_all(&bytes).expect("write voices");
    }

    #[test]
    fn load_voice_embedding_first_voice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("voices.bin");
        write_voices(&path, 4);

        let embedding = load_voice_embedding(&path, "").expect("load default voice");
        assert_eq!(embedding.len(), VOICE_DIM);
        assert_eq!(embedding[0], 0.0);
        assert_eq!(embedding[1], 1.0);
    }

    #[test]
    fn load_voice_embedding_named_voice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("voices.bin");
        write_voices(&path, 4);

        // af_heart is index 3.
        let embedding = load_voice_embedding(&path, "af_heart").expect("load af_heart");
        assert_eq!(embedding.len(), VOICE_DIM);
        assert_eq!(embedding[0], (3 * VOICE_DIM) as f32);
    }

    #[test]
    fn load_voice_embedding_unknown_voice_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("voices.bin");
        write_voices(&path, 4);

        let err = load_voice_embedding(&path, "not_a_voice").expect_err("unknown voice");
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn load_voice_embedding_rejects_non_float_aligned_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("voices.bin");
        std::fs::write(&path, [0_u8, 1, 2]).expect("write 3 bytes");

        let err = load_voice_embedding(&path, "").expect_err("non-aligned bytes");
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn load_voice_embedding_missing_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.bin");
        let err = load_voice_embedding(&path, "").expect_err("missing file");
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn voice_index_known_and_unknown() {
        assert_eq!(voice_index("af_heart"), Some(3));
        assert_eq!(voice_index("am_adam"), Some(11));
        assert_eq!(voice_index("nope"), None);
    }

    #[test]
    fn g2p_tokens_are_bos_eos_wrapped() {
        let tokens = crate::g2p::text_to_tokens("hello", Some("en"));
        assert_eq!(tokens.first().copied(), Some(0));
        assert_eq!(tokens.last().copied(), Some(0));
    }
}
