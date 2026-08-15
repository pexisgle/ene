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
//! snapshot when the singleton has no blob. The blob is the canonical
//! provider-owned config: `ai.tts` only routes the provider kind, so no
//! host-side merge happens on the request path.
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
use base64::Engine as _;
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

/// Created by [`IpcTtsProviderFactory`](crate::tts_factory::IpcTtsProviderFactory)
/// during `PluginHostManager` startup.
pub struct IpcTtsProvider {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    format: String,
    /// Provider config snapshot from `create_provider`, used when the global
    /// config singleton carries no blob for this plugin (see module docs).
    config_snapshot: serde_json::Value,
    /// Shared with every other provider instance the owning factory creates,
    /// so the concurrency bound holds across instances.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcTtsProvider {
    /// `limiter` should be the same `Arc<ConcurrencyLimiter>` shared by every
    /// provider instance the owning factory creates for this (plugin, kind)
    /// pair — see the module docs.
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

    /// Builds the provider config for the next synthesize: the live plugin
    /// blob (or the creation-time snapshot). The blob is the single source of
    /// provider-owned settings; `ai.tts` contributes only the routing kind.
    fn current_provider_config(&self) -> serde_json::Value {
        ene_ai::plugin_config::global_plugin_config_blob(&self.plugin_name)
            .unwrap_or_else(|| self.config_snapshot.clone())
    }
}

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

        let provider_config = self.current_provider_config();
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
                    String::new(),
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
            // Reject the base64 payload before decoding: a misbehaving
            // plugin could otherwise make the host allocate arbitrarily.
            // `max_base64_len` is the base64 length of [`wav::MAX_WAV_BYTES`].
            let max_base64_len = wav::MAX_WAV_BYTES.div_ceil(3) * 4;
            if audio_base64.len() > max_base64_len {
                drop(
                    tx.send(Err(AudioProviderError::PayloadTooLarge {
                        max_bytes: wav::MAX_WAV_BYTES,
                        actual: audio_base64.len() / 4 * 3,
                    }))
                    .await,
                );
                return;
            }
            let audio = match base64::engine::general_purpose::STANDARD.decode(audio_base64) {
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

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit tests use expect/unwrap for concise assertions"
)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use base64::Engine as _;
    use ene_plugin_proto::ConcurrencyHint;
    use ene_plugin_proto::{
        IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
        PluginIpcResponse, SandboxConfigData, WireFormat, cleanup_path, read_plugin_request,
        write_plugin_response,
    };
    use tokio::sync::Mutex;
    use tokio_stream::StreamExt as _;

    use super::*;
    use crate::ipc_provider::ConcurrencyLimiter;

    /// A scripted fake TTS plugin: completes the handshake, then answers
    /// every other request through `responder`.
    async fn run_mock_tts_server(
        socket_path: PathBuf,
        responder: Arc<dyn Fn(PluginIpcRequest) -> PluginIpcResponse + Send + Sync>,
    ) {
        cleanup_path(&socket_path);
        let Ok(mut listener) = IpcListener::bind(&socket_path) else {
            return;
        };
        loop {
            let Ok(stream) = listener.accept().await else {
                break;
            };
            let responder = Arc::clone(&responder);
            tokio::spawn(async move {
                let (mut read_half, write_half) = tokio::io::split(stream);
                let writer = Arc::new(Mutex::new(write_half));
                let mut format = WireFormat::Json;
                loop {
                    let Ok(Some(req)) = read_plugin_request(&mut read_half, format).await else {
                        break;
                    };
                    let resp_format = if matches!(&req, PluginIpcRequest::Handshake { .. }) {
                        format = WireFormat::for_version(PLUGIN_IPC_PROTOCOL_VERSION);
                        WireFormat::Json
                    } else {
                        format
                    };
                    let writer = Arc::clone(&writer);
                    let responder = Arc::clone(&responder);
                    tokio::spawn(async move {
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
                            other => responder(other),
                        };
                        let mut w = writer.lock().await;
                        drop(write_plugin_response(&mut *w, &resp, resp_format).await);
                    });
                }
            });
        }
    }

    async fn spawn_connected_provider(
        responder: impl Fn(PluginIpcRequest) -> PluginIpcResponse + Send + Sync + 'static,
        limiter: Arc<ConcurrencyLimiter>,
    ) -> (
        IpcTtsProvider,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket_path = dir.path().join("voicevox.sock");
        let server = tokio::spawn(run_mock_tts_server(
            socket_path.clone(),
            Arc::new(responder),
        ));
        let conn = IpcPluginConnection::connect(
            &socket_path,
            SandboxConfigData::default(),
            None,
            None,
            Duration::from_secs(5),
            4,
        )
        .await
        .expect("connect to mock plugin");
        let provider = IpcTtsProvider::new(
            "voicevox".to_string(),
            Arc::new(conn),
            "voicevox".to_string(),
            "wav".to_string(),
            serde_json::Value::Null,
            limiter,
        );
        (provider, server, dir)
    }

    fn default_limiter() -> Arc<ConcurrencyLimiter> {
        Arc::new(ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 2,
        }))
    }

    fn speech_result(request_id: &str, audio: &[u8]) -> PluginIpcResponse {
        PluginIpcResponse::SpeechResult {
            request_id: request_id.to_string(),
            audio_base64: base64::engine::general_purpose::STANDARD.encode(audio),
            format: "wav".to_string(),
        }
    }

    fn synthesize_request_id(req: &PluginIpcRequest) -> String {
        match req {
            PluginIpcRequest::SynthesizeSpeech { request_id, .. } => request_id.clone(),
            other => panic!("unexpected request: {other:?}"),
        }
    }

    /// Builds a mono s16 WAV whose PCM samples are all `1` (i.e. the f32
    /// value `1.0 / 32768.0`).
    fn wav_fixture(sample_count: usize, sample_rate: u32) -> Vec<u8> {
        let data_len = u32::try_from(sample_count * 2).expect("fixture fits in u32");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        let mut data = vec![0u8; data_len as usize];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = u8::from(i % 2 == 0);
        }
        bytes.extend_from_slice(&data);
        bytes
    }

    async fn collect_chunks(
        stream: Pin<Box<dyn Stream<Item = Result<TtsChunk, AudioProviderError>> + Send>>,
    ) -> Result<Vec<TtsChunk>, AudioProviderError> {
        let mut chunks = Vec::new();
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            chunks.push(chunk?);
        }
        Ok(chunks)
    }

    #[tokio::test]
    async fn synthesize_stream_round_trips_wav_and_slices_chunks() {
        let wav = wav_fixture(14_000, 24_000);
        let (provider, _server, _dir) = spawn_connected_provider(
            move |req| {
                let request_id = synthesize_request_id(&req);
                speech_result(&request_id, &wav)
            },
            default_limiter(),
        )
        .await;

        let chunks = collect_chunks(provider.synthesize_stream("hello").await.expect("stream"))
            .await
            .expect("synthesis succeeds");

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].pcm.len(), CHUNK_SAMPLES);
        assert_eq!(chunks[1].pcm.len(), CHUNK_SAMPLES);
        assert_eq!(chunks[2].pcm.len(), 2_000);
        let expected_sample = 1.0 / f32::from(i16::MAX);
        for chunk in &chunks {
            assert_eq!(chunk.sample_rate, 24_000);
            assert!(chunk.pcm.iter().all(|s| (s - expected_sample).abs() < 1e-9));
        }
    }

    #[tokio::test]
    async fn non_wav_base64_maps_to_unsupported_format() {
        let (provider, _server, _dir) = spawn_connected_provider(
            |req| {
                let request_id = synthesize_request_id(&req);
                speech_result(&request_id, b"definitely not a wav")
            },
            default_limiter(),
        )
        .await;

        let err = collect_chunks(provider.synthesize_stream("hello").await.expect("stream"))
            .await
            .expect_err("garbage audio rejected");
        assert!(matches!(err, AudioProviderError::UnsupportedFormat(_)));
    }

    #[tokio::test]
    async fn oversized_base64_is_rejected_before_decoding() {
        let huge = "A".repeat(wav::MAX_WAV_BYTES.div_ceil(3) * 4 + 1);
        let (provider, _server, _dir) = spawn_connected_provider(
            move |req| {
                let request_id = synthesize_request_id(&req);
                PluginIpcResponse::SpeechResult {
                    request_id,
                    audio_base64: huge.clone(),
                    format: "wav".to_string(),
                }
            },
            default_limiter(),
        )
        .await;

        let err = collect_chunks(provider.synthesize_stream("hello").await.expect("stream"))
            .await
            .expect_err("oversized payload rejected");
        assert!(matches!(
            err,
            AudioProviderError::PayloadTooLarge { max_bytes, .. }
                if max_bytes == wav::MAX_WAV_BYTES
        ));
    }

    #[tokio::test]
    async fn plugin_error_maps_to_provider_error() {
        let (provider, _server, _dir) = spawn_connected_provider(
            |req| {
                let request_id = synthesize_request_id(&req);
                PluginIpcResponse::Error {
                    request_id,
                    message: "engine exploded".to_string(),
                }
            },
            default_limiter(),
        )
        .await;

        let err = collect_chunks(provider.synthesize_stream("hello").await.expect("stream"))
            .await
            .expect_err("plugin error surfaces");
        assert!(matches!(err, AudioProviderError::Provider(_)));
        assert!(err.to_string().contains("engine exploded"));
    }

    #[tokio::test]
    async fn busy_is_propagated_when_limiter_is_exhausted() {
        let limiter = Arc::new(ConcurrencyLimiter::new(ConcurrencyHint {
            max_in_flight: 1,
            queue_depth: 0,
        }));
        let _permit = limiter.acquire("voicevox").await.expect("first permit");
        let (provider, _server, _dir) =
            spawn_connected_provider(|_| panic!("no request should be sent"), limiter).await;

        let Err(err) = provider.synthesize_stream("hello").await else {
            panic!("second caller should be rejected");
        };
        assert!(matches!(err, AudioProviderError::Busy { queue_depth: 0 }));
    }

    #[test]
    fn timeout_execution_failures_map_to_timeout() {
        let err = map_host_error(PluginHostError::execution(
            "timed out after 5000 ms waiting for a connection slot",
        ));
        assert!(matches!(err, AudioProviderError::Timeout));
    }

    #[test]
    fn transport_failures_map_to_provider_error() {
        let err = map_host_error(PluginHostError::transport("broken pipe"));
        assert!(matches!(err, AudioProviderError::Provider(_)));
        assert!(err.to_string().contains("transport"));
    }

    #[tokio::test]
    async fn factory_builds_provider_with_kind_and_plugin_blob() {
        use ene_ai::traits::TtsProviderFactory as _;

        let mut config = ene_config::EneConfig::default();
        drop(config.set_path("ai.tts.provider", "voicevox"));
        drop(config.set_path("plugins.list.voicevox.config.speaker_id", "7"));

        let (provider, _server, _dir) = spawn_connected_provider(
            move |req| {
                let request_id = synthesize_request_id(&req);
                speech_result(&request_id, &wav_fixture(1_000, 24_000))
            },
            default_limiter(),
        )
        .await;
        let factory = crate::tts_factory::IpcTtsProviderFactory::new(
            "voicevox".to_string(),
            provider.conn.clone(),
            "voicevox".to_string(),
            ConcurrencyHint {
                max_in_flight: 1,
                queue_depth: 2,
            },
        );

        let built = factory
            .create_provider(&config)
            .expect("factory builds provider");
        assert_eq!(built.name(), "voicevox");
    }
}
