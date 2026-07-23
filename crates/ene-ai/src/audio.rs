//! Audio provider traits, types, and registry for TTS, STT, and VAD.
//!
//! Mirrors the [`LlmProvider`](crate::traits::LlmProvider) /
//! [`LlmProviderRegistry`](crate::traits::LlmProviderRegistry) pattern with
//! separate factory maps for each audio modality.

use async_trait::async_trait;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
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
pub trait VadEngine: Send {
    /// Process a chunk of PCM audio and return the current VAD event.
    fn process_chunk(&mut self, pcm: &[f32]) -> VadEvent;

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
/// maps per modality.
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
        if let Ok(mut guard) = Self::global().tts.lock() {
            guard.insert(name, factory);
        }
    }

    /// Registers an STT provider factory.
    pub fn register_stt(factory: Arc<dyn SttProviderFactory>) {
        let name = factory.provider_name().to_string();
        if let Ok(mut guard) = Self::global().stt.lock() {
            guard.insert(name, factory);
        }
    }

    /// Registers a VAD engine factory.
    pub fn register_vad(factory: Arc<dyn VadFactory>) {
        let name = factory.provider_name().to_string();
        if let Ok(mut guard) = Self::global().vad.lock() {
            guard.insert(name, factory);
        }
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
        let factory = {
            if let Ok(guard) = Self::global().tts.lock() {
                guard.get(name).cloned()
            } else {
                None
            }
        };
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
        let factory = {
            if let Ok(guard) = Self::global().stt.lock() {
                guard.get(name).cloned()
            } else {
                None
            }
        };
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
        let factory = {
            if let Ok(guard) = Self::global().vad.lock() {
                guard.get(name).cloned()
            } else {
                None
            }
        };
        match factory {
            Some(f) => f.create_engine(config),
            None => Err(AudioProviderError::Provider(format!(
                "No VadFactory registered for provider name: '{name}'"
            ))),
        }
    }
}
