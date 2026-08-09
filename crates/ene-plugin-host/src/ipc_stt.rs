//! IPC-backed STT provider bridging the plugin wire protocol to `ene_ai::SttProvider`.
//!
//! [`IpcSttProvider`] holds a shared connection to a plugin binary and
//! translates [`SttProvider::transcribe`] calls into a single
//! `TranscribeAudio` IPC round trip. The wire contract takes a whole audio
//! file (base64), so the host encodes the microphone PCM into WAV before
//! sending — the mirror image of the TTS adapter's decode step.
//!
//! ## Live configuration
//!
//! Like TTS providers, each `transcribe` re-reads the plugin blob from the
//! global config singleton and falls back to the `create_provider` snapshot,
//! so `plugins.list.<name>.config` edits apply without a host restart.

use std::sync::Arc;

use async_trait::async_trait;
use ene_ai::AudioProviderError;
use ene_ai::traits::{SttProvider, SttResult};

use crate::error::PluginHostError;
use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::ConcurrencyLimiter;
use crate::wav;
use base64::Engine as _;

/// Audio format the host adapter can encode microphone PCM into. The wire
/// contract carries the format echo so a future provider that accepts
/// another container can be added with a matching encoder.
pub(crate) const STT_AUDIO_FORMAT: &str = "wav";

/// An `ene_ai::SttProvider` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcSttProviderFactory`](crate::stt_factory::IpcSttProviderFactory)
/// during `PluginHostManager` startup.
pub struct IpcSttProvider {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    format: String,
    /// Provider config snapshot from `create_provider`, used when the global
    /// config singleton carries no blob for this plugin.
    config_snapshot: serde_json::Value,
    /// Shared with every other provider instance the owning factory creates,
    /// so the concurrency bound holds across instances.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcSttProvider {
    /// Creates a new IPC-backed STT provider.
    ///
    /// `limiter` should be the same `Arc<ConcurrencyLimiter>` shared by every
    /// provider instance the owning factory creates for this (plugin, kind)
    /// pair — see the TTS adapter's docs for the reasoning.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        format: String,
        config_snapshot: serde_json::Value,
        limiter: Arc<ConcurrencyLimiter>,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            format,
            config_snapshot,
            limiter,
        }
    }

    /// Returns the live plugin blob (or the creation-time snapshot) merged
    /// with the resolved `ai.stt.model` / `ai.stt.language`, mirroring how
    /// the TTS adapter forwards `ai.tts.voice` so the desktop settings keep
    /// working without duplicating them in the plugin config.
    fn current_provider_config(&self) -> serde_json::Value {
        let mut blob = ene_ai::plugin_config::global_plugin_config_blob(&self.plugin_name)
            .unwrap_or_else(|| self.config_snapshot.clone());
        if let Ok(ai) = ene_config::get_global_config().get_section::<ene_ai::AiConfig>()
            && let Some(resolved) = ai.resolve_stt()
            && resolved.provider == self.kind
        {
            if !resolved.model.trim().is_empty() {
                blob["model"] = serde_json::Value::String(resolved.model.clone());
            }
            if let Some(language) = &resolved.language {
                blob["language"] = serde_json::Value::String(language.clone());
            }
        }
        blob
    }
}

/// Maps a [`PluginHostError`] into the [`AudioProviderError`] domain.
///
/// Mirrors the TTS adapter's mapping: IPC timeouts surface as `Timeout`,
/// transport failures are reported as provider errors and never retried
/// (the connection layer deliberately does not retry non-idempotent calls).
fn map_host_error(e: PluginHostError) -> AudioProviderError {
    match e {
        PluginHostError::TransportFailed { message } => {
            AudioProviderError::Provider(format!("plugin STT transport failed: {message}"))
        }
        PluginHostError::ExecutionFailed { message } if message.contains("timed out") => {
            AudioProviderError::Timeout
        }
        other => AudioProviderError::Provider(other.to_string()),
    }
}

#[async_trait]
impl SttProvider for IpcSttProvider {
    fn name(&self) -> &str {
        &self.kind
    }

    async fn transcribe(
        &self,
        pcm: &[f32],
        sample_rate: u32,
    ) -> Result<SttResult, AudioProviderError> {
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

        let wav_bytes = wav::encode_wav(pcm, sample_rate)?;
        let audio_base64 = base64::engine::general_purpose::STANDARD.encode(wav_bytes);
        let outcome = self
            .conn
            .transcribe_audio(
                String::new(),
                self.kind.clone(),
                self.current_provider_config(),
                audio_base64,
                self.format.clone(),
            )
            .await;
        drop(permit);

        let (text, language) = match outcome {
            Ok(result) => result,
            Err(e) => return Err(map_host_error(e)),
        };
        Ok(SttResult {
            text,
            language,
            duration_secs: (pcm.len() as f32) / (sample_rate.max(1) as f32),
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use ene_plugin_proto::{
        ConcurrencyHint, IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities,
        PluginIpcRequest, PluginIpcResponse, WireFormat, cleanup_path, read_plugin_request,
        write_plugin_response,
    };
    use tokio::sync::Mutex;

    use super::*;
    use crate::ipc_provider::ConcurrencyLimiter;

    /// A scripted fake STT plugin: completes the handshake, then answers
    /// `TranscribeAudio` with a fixed transcript.
    async fn run_mock_stt_server(socket_path: PathBuf) {
        cleanup_path(&socket_path);
        let Ok(mut listener) = IpcListener::bind(&socket_path) else {
            return;
        };
        loop {
            let Ok(stream) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let (mut read_half, write_half) = tokio::io::split(stream);
                let writer = Arc::new(Mutex::new(write_half));
                let mut format = WireFormat::Json;
                while let Ok(Some(req)) = read_plugin_request(&mut read_half, format).await {
                    let resp_format = if matches!(&req, PluginIpcRequest::Handshake { .. }) {
                        format = WireFormat::for_version(PLUGIN_IPC_PROTOCOL_VERSION);
                        WireFormat::Json
                    } else {
                        format
                    };
                    let resp = match req {
                        PluginIpcRequest::Handshake { .. } => PluginIpcResponse::HandshakeAck {
                            version: PLUGIN_IPC_PROTOCOL_VERSION,
                            capabilities: PluginCapabilities {
                                tools: 0,
                                llm_providers: Vec::new(),
                                tts_providers: Vec::new(),
                                stt_providers: Vec::new(),
                                ..PluginCapabilities::default()
                            },
                        },
                        PluginIpcRequest::TranscribeAudio { request_id, .. } => {
                            PluginIpcResponse::TranscriptionResult {
                                request_id,
                                text: "hello world".into(),
                                language: Some("en".into()),
                            }
                        }
                        other => PluginIpcResponse::Error {
                            request_id: String::new(),
                            message: format!("unexpected request: {other:?}"),
                        },
                    };
                    let mut w = writer.lock().await;
                    drop(write_plugin_response(&mut *w, &resp, resp_format).await);
                }
            });
        }
    }

    async fn spawn_provider() -> (
        IpcSttProvider,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("stt.sock");
        let server = tokio::spawn(run_mock_stt_server(socket_path.clone()));
        let conn = crate::ipc_plugin::IpcPluginConnection::connect(
            &socket_path,
            ene_plugin_proto::SandboxConfigData::default(),
            None,
            None,
            std::time::Duration::from_secs(5),
            4,
        )
        .await
        .expect("connect to mock STT plugin");
        let provider = IpcSttProvider::new(
            "whisper".into(),
            Arc::new(conn),
            "whisper".into(),
            STT_AUDIO_FORMAT.to_string(),
            serde_json::Value::Null,
            Arc::new(ConcurrencyLimiter::new(ConcurrencyHint::default())),
        );
        (provider, server, dir)
    }

    #[tokio::test]
    async fn transcribe_roundtrips_text_and_derives_duration() {
        let (provider, server, _dir) = spawn_provider().await;
        let pcm = vec![0.0_f32; 16_000];
        let result = provider.transcribe(&pcm, 16_000).await.expect("transcribe");
        assert_eq!(result.text, "hello world");
        assert_eq!(result.language.as_deref(), Some("en"));
        assert!((result.duration_secs - 1.0).abs() < 1e-4);
        server.abort();
    }
}
