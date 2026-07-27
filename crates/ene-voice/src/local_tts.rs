//! Local text-to-speech provider backed by Kokoro ONNX (`ort`).
//!
//! The heavy native dependency is gated behind the `local-tts` cargo feature.
//! When the feature is disabled, [`LocalTtsProviderFactory`] still registers
//! but fails fast with [`AudioProviderError::Init`] so the crate (and the
//! workspace) keeps compiling without the ONNX Runtime toolchain.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "voice indexing and PCM chunk arithmetic use bounded counters"
)]
#![allow(
    clippy::indexing_slicing,
    reason = "voice embedding slicing indexes into bounds-checked `voices.bin` floats"
)]

#[cfg(feature = "local-tts")]
use async_trait::async_trait;
#[cfg(feature = "local-tts")]
use ene_ai::TtsChunk;
use ene_ai::{AudioProviderError, AudioProviderRegistry, TtsProvider, TtsProviderFactory};
#[cfg(feature = "local-tts")]
use std::pin::Pin;
#[cfg(feature = "local-tts")]
use std::sync::Arc;
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

/// Dimensionality of a single Kokoro voice style embedding.
#[cfg(feature = "local-tts")]
const VOICE_DIM: usize = 256;

/// Known Kokoro-82M (v1.0) voice names mapped to their stacked index in
/// `voices.bin` (C5). `voices.bin` is a flat little-endian `f32` array of
/// `VOICE_DIM`-float vectors concatenated in this order.
#[cfg(feature = "local-tts")]
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

/// Resolve the Kokoro ONNX model path from configuration.
///
/// Precedence: `TtsConfig::model_path` when non-empty, then `TtsConfig::model`
/// when non-empty, then a default cache location. Environment overrides are
/// handled by the config system (`ENE_AI__TTS__MODEL_PATH`).
#[cfg(feature = "local-tts")]
fn resolve_model_path(ai: &ene_ai::AiConfig) -> std::path::PathBuf {
    if let Some(path) = ai
        .tts
        .model_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    if !ai.tts.model.trim().is_empty() {
        return std::path::PathBuf::from(ai.tts.model.trim());
    }
    ene_config::models_dir().join("gguf").join("kokoro.onnx")
}

/// Resolve the Kokoro `voices.bin` path from configuration.
///
/// Precedence: `TtsConfig::voices_path` when non-empty, then a default cache
/// location. Environment overrides are handled by the config system
/// (`ENE_AI__TTS__VOICES_PATH`).
#[cfg(feature = "local-tts")]
fn resolve_voices_path(ai: &ene_ai::AiConfig) -> std::path::PathBuf {
    if let Some(path) = ai
        .tts
        .voices_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return std::path::PathBuf::from(path);
    }
    ene_config::models_dir().join("gguf").join("voices.bin")
}

/// Local Kokoro ONNX text-to-speech provider.
#[cfg(feature = "local-tts")]
pub struct LocalTtsProvider {
    session: Arc<parking_lot::Mutex<ort::session::Session>>,
    voice: Vec<f32>,
    speed: f32,
    language: Option<String>,
}

#[cfg(feature = "local-tts")]
impl LocalTtsProvider {
    /// Load the Kokoro ONNX model and the selected voice embedding.
    ///
    /// ONNX Runtime is initialized exactly once per process (C3) using
    /// `ort_dylib_path` when provided. The voice named by `voice_name` (e.g.
    /// `"af_heart"`) is selected from `voices.bin`; an empty name selects the
    /// first voice.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Init`] when the model or voice file is
    /// missing, the requested voice is unknown, or ONNX Runtime fails to build
    /// a session.
    pub fn open(
        model_path: &std::path::Path,
        voices_path: &std::path::Path,
        voice_name: &str,
        speed: f32,
        language: Option<String>,
        ort_dylib_path: Option<&str>,
    ) -> Result<Self, AudioProviderError> {
        crate::ort_init::ensure_ort_init(ort_dylib_path)?;

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

        let voice = load_voice_embedding(voices_path, voice_name)?;

        tracing::info!(
            component = "LocalTts",
            model = %model_path.display(),
            voices = %voices_path.display(),
            voice = voice_name,
            "loaded Kokoro ONNX model"
        );
        Ok(Self {
            session: Arc::new(parking_lot::Mutex::new(session)),
            voice,
            speed,
            language,
        })
    }

    /// Run Kokoro inference over pre-tokenized phoneme ids and return the full
    /// PCM buffer (blocking).
    fn run_inference(
        session: &mut ort::session::Session,
        voice: &[f32],
        tokens: &[i64],
        speed: f32,
    ) -> Result<Vec<f32>, AudioProviderError> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let seq_len = tokens.len();

        let style = voice.to_vec();
        let token_buf = tokens.to_vec();
        let outputs = session
            .run(ort::inputs![
                "input_ids" => ort::value::Tensor::from_array((
                    [1_i64, seq_len as i64],
                    token_buf,
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

/// Look up the stacked `voices.bin` index for a Kokoro voice name.
#[cfg(feature = "local-tts")]
fn voice_index(name: &str) -> Option<usize> {
    KOKORO_VOICES
        .iter()
        .find(|(voice, _)| *voice == name)
        .map(|(_, idx)| *idx)
}

/// Load a single `VOICE_DIM`-dim voice embedding for `voice_name` from
/// `voices.bin`.
///
/// `voices.bin` is a flat little-endian `f32` array of stacked voice vectors
/// (see [`KOKORO_VOICES`]). An empty `voice_name` selects the first voice.
///
/// # Errors
///
/// Returns [`AudioProviderError::Init`] when the file is missing, its byte
/// length is not a multiple of four (M16), it is too small for the requested
/// voice, or `voice_name` is not a known voice.
#[cfg(feature = "local-tts")]
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

#[cfg(feature = "local-tts")]
#[async_trait]
impl TtsProvider for LocalTtsProvider {
    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    /// Deliver synthesized PCM as fixed-size chunks (H10).
    ///
    /// This is chunked delivery of batch inference results via mpsc, not true
    /// streaming synthesis: Kokoro runs a single batch inference over all
    /// phoneme tokens on a blocking thread, and the resulting PCM buffer is
    /// then sliced into [`CHUNK_SAMPLES`]-sized chunks pushed through the
    /// channel as the consumer polls them.
    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    > {
        // Convert text to Kokoro phoneme token ids (C4) before inference.
        let tokens = crate::g2p::text_to_tokens(text, self.language.as_deref());
        // `text_to_tokens` always emits BOS/EOS; anything beyond that is audio.
        if tokens.len() <= 2 {
            return Ok(Box::pin(tokio_stream::empty::<
                Result<TtsChunk, AudioProviderError>,
            >()));
        }

        let session = Arc::clone(&self.session);
        let voice = self.voice.clone();
        let speed = self.speed;

        // Chunked delivery of batch inference results via mpsc (H10): a single
        // batch inference runs on a blocking thread, then the full PCM buffer
        // is pushed as fixed-size chunks through the channel. This is not true
        // token-by-token streaming synthesis.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<TtsChunk, AudioProviderError>>(4);
        tokio::task::spawn_blocking(move || {
            let result = {
                let mut guard = session.lock();
                Self::run_inference(&mut guard, &voice, &tokens, speed)
            };
            match result {
                Ok(pcm) => {
                    for slice in pcm.chunks(CHUNK_SAMPLES) {
                        let chunk = TtsChunk {
                            pcm: slice.to_vec(),
                            sample_rate: KOKORO_SAMPLE_RATE,
                        };
                        // Stop if the consumer dropped the receiver.
                        if tx.blocking_send(Ok(chunk)).is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    drop(tx.blocking_send(Err(e)));
                }
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

/// Factory for the local Kokoro ONNX TTS provider.
pub struct LocalTtsProviderFactory;

#[cfg(feature = "local-tts")]
impl LocalTtsProviderFactory {
    fn build(config: &ene_config::EneConfig) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        let ai = config
            .get_section::<ene_ai::AiConfig>()
            .map_err(|e| AudioProviderError::Init(format!("failed to parse AI config: {e}")))?;
        let resolved = ai.resolve_tts().ok_or_else(|| {
            AudioProviderError::Init(
                "TTS provider is disabled (ai.tts.provider = \"none\")".to_string(),
            )
        })?;
        let model_path = resolve_model_path(&ai);
        let voices_path = resolve_voices_path(&ai);
        let voice_name = resolved.voice.clone().unwrap_or_default();
        let provider = LocalTtsProvider::open(
            &model_path,
            &voices_path,
            &voice_name,
            resolved.speed,
            resolved.language.clone(),
            ai.ort_dylib_path.as_deref(),
        )?;
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

#[cfg(all(test, feature = "local-tts"))]
mod tests {
    #![allow(
        clippy::float_cmp,
        reason = "voice embeddings are written and read back as exact f32 bit patterns"
    )]

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
