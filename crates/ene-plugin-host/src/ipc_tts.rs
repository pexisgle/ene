//! IPC-backed TTS provider bridging the plugin wire protocol to `ene_ai::TtsProvider`.
//!
//! [`IpcTtsProvider`] holds a shared connection to a plugin binary and
//! translates [`TtsProvider::synthesize_stream`] calls into a single
//! `SynthesizeSpeech` IPC round-trip. The wire contract returns a whole
//! audio file (base64 `SpeechResult`), not incremental audio, so the PCM is
//! decoded from WAV and sliced into fixed-size [`TtsChunk`]s on the host
//! side — the same shape `ene-ai::engine_adapter::tts` uses for one-shot
//! local synthesis.
//!
//! ## Live configuration
//!
//! TTS providers are long-lived (built once at runtime bootstrap, unlike
//! LLM providers which are rebuilt per call), so a config snapshot taken at
//! creation would go stale on the next `plugins.list.<name>.config` edit.
//! Each synthesize therefore re-reads the plugin blob from the global config
//! singleton (refreshed on every full config load — see
//! [`ene_ai::plugin_config`]) and falls back to the `create_provider`
//! snapshot when the singleton has no blob.
//!
//! ## Concurrency admission control
//!
//! The [`ConcurrencyLimiter`] shared by every provider instance a factory
//! creates enforces the
//! [`ConcurrencyHint`](ene_plugin_proto::ConcurrencyHint) the plugin
//! declared during the handshake, so the host cannot open unbounded
//! concurrent synthesis requests against a plugin that declared serial
//! operation.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use ene_ai::AudioProviderError;
use ene_ai::traits::{TtsChunk, TtsProvider};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;

use crate::error::PluginHostError;
use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::ConcurrencyLimiter;
use crate::wav;

/// PCM samples per streamed [`TtsChunk`]. Matches
/// `ene-ai::engine_adapter::tts::DEFAULT_CHUNK_SAMPLES` (~0.25 s at 24 kHz,
/// the VOICEVOX default output rate).
const CHUNK_SAMPLES: usize = 6_000;

/// An `ene_ai::TtsProvider` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcTtsProviderFactory`](crate::tts_factory::IpcTtsProviderFactory)
/// during `PluginHostManager` startup.
pub struct IpcTtsProvider {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    voice: Option<String>,
    format: String,
    /// Provider config snapshot from `create_provider`, used when the global
    /// config singleton carries no blob for this plugin (see module docs).
    config_snapshot: serde_json::Value,
    /// Shared with every other provider instance the owning factory creates,
    /// so the concurrency bound holds across instances.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcTtsProvider {
    /// Creates a new IPC-backed TTS provider.
    ///
    /// `limiter` should be the same `Arc<ConcurrencyLimiter>` shared by every
    /// provider instance the owning factory creates for this (plugin, kind)
    /// pair — see the module docs.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        voice: Option<String>,
        format: String,
        config_snapshot: serde_json::Value,
        limiter: Arc<ConcurrencyLimiter>,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            voice,
            format,
            config_snapshot,
            limiter,
        }
    }

    /// Builds the provider config for the next synthesize: the live plugin
    /// blob (or the creation-time snapshot) merged with the resolved
    /// `ai.tts.voice` so a speaker override set after startup also applies.
    fn current_provider_config(&self) -> (serde_json::Value, Option<String>) {
        let blob = ene_ai::plugin_config::global_plugin_config_blob(&self.plugin_name)
            .unwrap_or_else(|| self.config_snapshot.clone());
        let voice = ene_config::get_global_config()
            .get_section::<ene_ai::AiConfig>()
            .ok()
            .and_then(|ai| ai.resolve_tts())
            .filter(|resolved| resolved.provider == self.kind)
            .and_then(|resolved| resolved.voice)
            .or_else(|| self.voice.clone());
        (blob, voice)
    }
}

/// Maps a [`PluginHostError`] into the [`AudioProviderError`] domain.
///
/// IPC timeouts surface as `ExecutionFailed` with a "timed out" message (the
/// connection layer deliberately does not retry timeouts); transport
/// failures are reported as provider errors and never retried by the TTS
/// consumer, which mirrors the conservative stance the connection layer
/// already takes for non-idempotent calls.
fn map_host_error(e: PluginHostError) -> AudioProviderError {
    match e {
        PluginHostError::TransportFailed { message } => {
            AudioProviderError::Provider(format!("plugin TTS transport failed: {message}"))
        }
        PluginHostError::ExecutionFailed { message } if message.contains("timed out") => {
            AudioProviderError::Timeout
        }
        other => AudioProviderError::Provider(other.to_string()),
    }
}

#[async_trait]
impl TtsProvider for IpcTtsProvider {
    fn name(&self) -> &str {
        &self.kind
    }

    async fn synthesize_stream(
        &self,
        text: &str,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
        AudioProviderError,
    > {
        let permit = self
            .limiter
            .acquire(&self.kind)
            .await
            .map_err(|e| match e {
                ene_ai::LlmProviderError::Busy { queue_depth } => {
                    AudioProviderError::Busy { queue_depth }
                }
                other => AudioProviderError::Provider(other.to_string()),
            })?;

        let (provider_config, voice) = self.current_provider_config();
        let (tx, rx) = mpsc::channel::<Result<TtsChunk, AudioProviderError>>(4);
        let conn = Arc::clone(&self.conn);
        let kind = self.kind.clone();
        let format = self.format.clone();
        let text = text.to_string();

        tokio::spawn(async move {
            let outcome = conn
                .synthesize_speech(
                    String::new(),
                    kind,
                    provider_config,
                    text,
                    voice.unwrap_or_default(),
                    format,
                )
                .await;
            drop(permit);

            let (audio_base64, _) = match outcome {
                Ok(result) => result,
                Err(e) => {
                    drop(tx.send(Err(map_host_error(e))).await);
                    return;
                }
            };
            let audio = match base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                audio_base64,
            ) {
                Ok(bytes) => bytes,
                Err(e) => {
                    drop(
                        tx.send(Err(AudioProviderError::UnsupportedFormat(format!(
                            "plugin returned invalid base64 audio: {e}"
                        ))))
                        .await,
                    );
                    return;
                }
            };
            let decoded = match wav::decode_wav(&audio) {
                Ok(decoded) => decoded,
                Err(e) => {
                    drop(tx.send(Err(e)).await);
                    return;
                }
            };
            for chunk in decoded.pcm.chunks(CHUNK_SAMPLES) {
                if tx
                    .send(Ok(TtsChunk {
                        pcm: chunk.to_vec(),
                        sample_rate: decoded.sample_rate,
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}
