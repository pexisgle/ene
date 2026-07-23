//! Audio provider traits, types, and registry for TTS, STT, and VAD.
//!
//! Mirrors the [`LlmProvider`](crate::traits::LlmProvider) /
//! [`LlmProviderRegistry`](crate::traits::LlmProviderRegistry) pattern with
//! separate factory maps for each audio modality.

use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio_stream::{Stream, StreamExt};

/// Errors returned by audio provider implementations at the library boundary.
#[derive(Debug, thiserror::Error)]
pub enum AudioProviderError {
    /// The audio backend failed to initialize (model load, device open).
    #[error("audio init error: {0}")]
    Init(String),
    /// The provider encountered an error during synthesis or transcription.
    #[error("audio provider error: {0}")]
    Provider(String),
    /// The requested audio format or sample rate is not supported.
    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),
    /// The operation timed out.
    #[error("audio operation timed out")]
    Timeout,
    /// An underlying I/O operation failed (file read, dylib load).
    #[error("audio I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// A chunk of synthesized PCM audio from a TTS provider.
#[derive(Debug, Clone, PartialEq)]
pub struct TtsChunk {
    /// Interleaved mono PCM samples normalized to `[-1.0, 1.0]`.
    pub pcm: Vec<f32>,
    /// Sample rate in Hz (e.g. 24000).
    pub sample_rate: u32,
}

/// Trait implemented by text-to-speech providers.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Provider display name (e.g. `"kokoro"`, `"openai"`).
    fn name(&self) -> &str;

    /// Synthesize text into a stream of PCM audio chunks.
    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    >;

    /// Synthesize text and collect all PCM chunks.
    ///
    /// Default implementation collects [`synthesize_stream`](Self::synthesize_stream).
    async fn synthesize(&self, text: &str) -> Result<Vec<TtsChunk>, AudioProviderError> {
        let mut stream = self.synthesize_stream(text).await?;
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk?);
        }
        Ok(chunks)
    }
}

/// Result of a speech-to-text transcription.
#[derive(Debug, Clone, PartialEq)]
pub struct SttResult {
    /// Transcribed text.
    pub text: String,
    /// Detected language code (e.g. `"ja"`, `"en"`), if available.
    pub language: Option<String>,
    /// Duration of the transcribed audio in seconds.
    pub duration_secs: f32,
}

/// Trait implemented by speech-to-text providers.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Provider display name (e.g. `"whisper"`, `"openai"`).
    fn name(&self) -> &str;

    /// Transcribe PCM audio to text.
    async fn transcribe(
        &self,
        pcm: &[f32],
        sample_rate: u32,
    ) -> Result<SttResult, AudioProviderError>;
}

/// Voice activity detection event emitted per processed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VadEvent {
    /// Speech just started.
    SpeechStart,
    /// Speech is continuing.
    SpeechContinue,
    /// Speech just ended.
    SpeechEnd,
    /// No speech detected.
    Silence,
}

/// Trait implemented by voice activity detection engines.
///
/// Engines are stateful (`&mut self`) and process fixed-size PCM chunks
/// sequentially. Not `Sync` because internal state is mutated per chunk.
///
/// # Frame-size contract
///
/// Each engine consumes frames of a fixed length reported by
/// [`frame_size`](Self::frame_size) (e.g. Silero VAD expects 512 samples at
/// 16 kHz / 32 ms). Callers must accumulate microphone PCM into exactly
/// [`frame_size`](Self::frame_size) samples before each
/// [`process_chunk`](Self::process_chunk) call. Passing a slice of any other
/// length is a programming error and [`process_chunk`](Self::process_chunk)
/// returns [`AudioProviderError::Provider`].
pub trait VadEngine: Send {
    /// Number of PCM samples expected per [`process_chunk`](Self::process_chunk) call.
    fn frame_size(&self) -> usize;

    /// Process a single [`frame_size`](Self::frame_size)-sample chunk of PCM
    /// audio and return the current VAD event.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Provider`] when `pcm.len()` does not equal
    /// [`frame_size`](Self::frame_size), or when the underlying inference step
    /// fails repeatedly (see the engine implementation for escalation policy).
    fn process_chunk(&mut self, pcm: &[f32]) -> Result<VadEvent, AudioProviderError>;

    /// Reset the engine to its initial state.
    fn reset(&mut self);

    /// Engine display name (e.g. `"silero"`, `"webrtc"`).
    fn name(&self) -> &str;
}

/// Factory trait to build [`TtsProvider`] instances from workspace configs.
pub trait TtsProviderFactory: Send + Sync {
    /// The unique name of the provider this factory produces.
    fn provider_name(&self) -> &str;

    /// Instantiates the provider based on current `EneConfig` settings.
    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn TtsProvider>, AudioProviderError>;
}

/// Factory trait to build [`SttProvider`] instances from workspace configs.
pub trait SttProviderFactory: Send + Sync {
    /// The unique name of the provider this factory produces.
    fn provider_name(&self) -> &str;

    /// Instantiates the provider based on current `EneConfig` settings.
    fn create_provider(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn SttProvider>, AudioProviderError>;
}

/// Factory trait to build [`VadEngine`] instances from workspace configs.
pub trait VadFactory: Send + Sync {
    /// The unique name of the engine this factory produces.
    fn provider_name(&self) -> &str;

    /// Instantiates the engine based on current `EneConfig` settings.
    fn create_engine(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn VadEngine>, AudioProviderError>;
}

/// Global registry of audio provider factories (TTS, STT, VAD).
///
/// Uses the same `OnceLock` + `Mutex` pattern as
/// [`LlmProviderRegistry`](crate::traits::LlmProviderRegistry) with separate
/// maps per modality. Locks use [`parking_lot::Mutex`] (poison-free, so no
/// poisoning checks are required at the call sites).
pub struct AudioProviderRegistry {
    tts: Mutex<HashMap<String, Arc<dyn TtsProviderFactory>>>,
    stt: Mutex<HashMap<String, Arc<dyn SttProviderFactory>>>,
    vad: Mutex<HashMap<String, Arc<dyn VadFactory>>>,
}

impl AudioProviderRegistry {
    fn global() -> &'static Self {
        static REGISTRY: OnceLock<AudioProviderRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self {
            tts: Mutex::new(HashMap::new()),
            stt: Mutex::new(HashMap::new()),
            vad: Mutex::new(HashMap::new()),
        })
    }

    /// Registers a TTS provider factory.
    pub fn register_tts(factory: Arc<dyn TtsProviderFactory>) {
        let name = factory.provider_name().to_string();
        Self::global().tts.lock().insert(name, factory);
    }

    /// Registers an STT provider factory.
    pub fn register_stt(factory: Arc<dyn SttProviderFactory>) {
        let name = factory.provider_name().to_string();
        Self::global().stt.lock().insert(name, factory);
    }

    /// Registers a VAD engine factory.
    pub fn register_vad(factory: Arc<dyn VadFactory>) {
        let name = factory.provider_name().to_string();
        Self::global().vad.lock().insert(name, factory);
    }

    /// Tries to instantiate a TTS provider by name using the registered factories.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Provider`] if no factory is registered for
    /// `name`, or the factory's own error if initialization fails.
    pub fn create_tts_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
        let factory = Self::global().tts.lock().get(name).cloned();
        match factory {
            Some(f) => f.create_provider(config),
            None => Err(AudioProviderError::Provider(format!(
                "No TtsProviderFactory registered for provider name: '{name}'"
            ))),
        }
    }

    /// Tries to instantiate an STT provider by name using the registered factories.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Provider`] if no factory is registered for
    /// `name`, or the factory's own error if initialization fails.
    pub fn create_stt_provider(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn SttProvider>, AudioProviderError> {
        let factory = Self::global().stt.lock().get(name).cloned();
        match factory {
            Some(f) => f.create_provider(config),
            None => Err(AudioProviderError::Provider(format!(
                "No SttProviderFactory registered for provider name: '{name}'"
            ))),
        }
    }

    /// Tries to instantiate a VAD engine by name using the registered factories.
    ///
    /// # Errors
    ///
    /// Returns [`AudioProviderError::Provider`] if no factory is registered for
    /// `name`, or the factory's own error if initialization fails.
    pub fn create_vad_engine(
        name: &str,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn VadEngine>, AudioProviderError> {
        let factory = Self::global().vad.lock().get(name).cloned();
        match factory {
            Some(f) => f.create_engine(config),
            None => Err(AudioProviderError::Provider(format!(
                "No VadFactory registered for provider name: '{name}'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyTtsFactory;

    impl TtsProviderFactory for DummyTtsFactory {
        fn provider_name(&self) -> &'static str {
            "dummy-tts"
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn TtsProvider>, AudioProviderError> {
            Err(AudioProviderError::Init("dummy".to_string()))
        }
    }

    struct DummySttFactory;

    impl SttProviderFactory for DummySttFactory {
        fn provider_name(&self) -> &'static str {
            "dummy-stt"
        }

        fn create_provider(
            &self,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn SttProvider>, AudioProviderError> {
            Err(AudioProviderError::Init("dummy".to_string()))
        }
    }

    struct DummyVadFactory;

    impl VadFactory for DummyVadFactory {
        fn provider_name(&self) -> &'static str {
            "dummy-vad"
        }

        fn create_engine(
            &self,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn VadEngine>, AudioProviderError> {
            Err(AudioProviderError::Init("dummy".to_string()))
        }
    }

    fn config() -> ene_config::EneConfig {
        ene_config::EneConfig::default()
    }

    /// Assert that `result` is an `Err` and return it. Used instead of
    /// `expect_err` because the boxed provider types are not `Debug`.
    fn unwrap_err<T>(result: Result<T, AudioProviderError>) -> AudioProviderError {
        match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e,
        }
    }

    #[test]
    fn tts_registration_lookup_round_trip() {
        AudioProviderRegistry::register_tts(Arc::new(DummyTtsFactory));
        // Registered factory is found; its own init error surfaces (not the
        // "no factory registered" error).
        let err = unwrap_err(AudioProviderRegistry::create_tts_provider(
            "dummy-tts",
            &config(),
        ));
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn stt_registration_lookup_round_trip() {
        AudioProviderRegistry::register_stt(Arc::new(DummySttFactory));
        let err = unwrap_err(AudioProviderRegistry::create_stt_provider(
            "dummy-stt",
            &config(),
        ));
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn vad_registration_lookup_round_trip() {
        AudioProviderRegistry::register_vad(Arc::new(DummyVadFactory));
        let err = unwrap_err(AudioProviderRegistry::create_vad_engine(
            "dummy-vad",
            &config(),
        ));
        assert!(matches!(err, AudioProviderError::Init(_)));
    }

    #[test]
    fn unknown_provider_name_errors() {
        let err = unwrap_err(AudioProviderRegistry::create_tts_provider(
            "does-not-exist",
            &config(),
        ));
        assert!(matches!(err, AudioProviderError::Provider(_)));
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: AudioProviderError = io_err.into();
        assert!(matches!(err, AudioProviderError::Io(_)));
    }
}
