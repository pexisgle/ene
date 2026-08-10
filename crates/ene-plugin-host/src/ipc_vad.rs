//! IPC-backed VAD engine bridging the plugin wire protocol to `ene_ai::VadEngine`.
//!
//! [`IpcVadEngine`] is the one host adapter whose calls are *synchronous*:
//! `ene_ai::VadEngine::process_chunk` runs on the microphone capture thread
//! once per fixed-size frame (32 ms for Silero VAD), so the adapter bridges
//! to the async connection with `tokio::runtime::Handle::block_on`. One IPC
//! round trip per frame over a local socket is micro-to-low-millisecond
//! latency against a 32 ms frame cadence — comparable to the in-process ONNX
//! inference it replaces.
//!
//! ## Sessions
//!
//! VAD engine state (recurrent cell state, speech edge tracking) lives in
//! the plugin process, keyed by a host-generated `session_id`. Each
//! [`IpcVadEngine`] instance owns one id for its lifetime; `reset` clears
//! the plugin-side state, and dropping the engine sends a final `reset` so
//! repeated mic toggles do not leak ONNX sessions in the plugin process.
//! The factory's [`ConcurrencyLimiter`](crate::ipc_provider::ConcurrencyLimiter)
//! enforces the plugin's declared `ConcurrencyHint` across sessions (each
//! chunk holds a permit for its round trip).
//!
//! ## Runtime requirements
//!
//! `process_chunk` runs `Handle::block_on` from the capture thread, which
//! requires the captured runtime to make progress independently of that
//! thread: use a multi-thread runtime (the desktop's runtime is). A
//! current-thread runtime whose only driver is the blocked caller would
//! deadlock; the factory captures `Handle::current()` at manager startup,
//! so the requirement is on the host process, not per call.

use std::sync::Arc;

use ene_ai::AudioProviderError;
use ene_ai::traits::VadEngine;
use ene_ai::{VadEvent, VadFactory};
use ene_plugin_proto::ConcurrencyHint;
use ene_plugin_proto::VadEvent as WireVadEvent;

use crate::ipc_plugin::IpcPluginConnection;
use crate::ipc_provider::ConcurrencyLimiter;

/// Generates unique session ids for the plugin-side engine state map.
fn next_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!("vad-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Maps a wire VAD event onto the host's `ene_ai::VadEvent`.
fn map_event(event: WireVadEvent) -> VadEvent {
    match event {
        WireVadEvent::SpeechStart => VadEvent::SpeechStart,
        WireVadEvent::SpeechContinue => VadEvent::SpeechContinue,
        WireVadEvent::SpeechEnd => VadEvent::SpeechEnd,
        WireVadEvent::Silence => VadEvent::Silence,
    }
}

/// An `ene_ai::VadEngine` that delegates to a plugin binary over IPC.
///
/// Created by [`IpcVadFactory`] during `PluginHostManager` startup. Holds a
/// `tokio::runtime::Handle` captured at factory construction so
/// `process_chunk` can block on the async connection from the capture
/// thread.
pub struct IpcVadEngine {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    session_id: String,
    frame_size: usize,
    sample_rate: u32,
    config_snapshot: serde_json::Value,
    handle: tokio::runtime::Handle,
    /// Shared with every other engine instance the owning factory creates,
    /// so the plugin's declared concurrency bound holds across sessions.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcVadEngine {
    /// Creates a new IPC-backed VAD engine with a fresh session id.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        frame_size: usize,
        sample_rate: u32,
        config_snapshot: serde_json::Value,
        handle: tokio::runtime::Handle,
        limiter: Arc<ConcurrencyLimiter>,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            session_id: next_session_id(),
            frame_size,
            sample_rate,
            config_snapshot,
            handle,
            limiter,
        }
    }

    /// The PCM sample rate this engine's chunks arrive at (from the
    /// plugin's `VadProviderSpec`).
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Returns the live plugin blob (or the creation-time snapshot).
    fn current_provider_config(&self) -> serde_json::Value {
        ene_ai::plugin_config::global_plugin_config_blob(&self.plugin_name)
            .unwrap_or_else(|| self.config_snapshot.clone())
    }

    fn round_trip(&self, pcm: Vec<f32>, reset: bool) -> Result<VadEvent, AudioProviderError> {
        let conn = Arc::clone(&self.conn);
        let limiter = Arc::clone(&self.limiter);
        let kind = self.kind.clone();
        let config = self.current_provider_config();
        let session_id = self.session_id.clone();
        let outcome: Result<WireVadEvent, AudioProviderError> = self.handle.block_on(async move {
            let permit = limiter.acquire(&kind).await.map_err(|e| match e {
                ene_ai::LlmProviderError::Busy { queue_depth } => {
                    AudioProviderError::Busy { queue_depth }
                }
                other => AudioProviderError::Provider(other.to_string()),
            })?;
            let result = conn
                .process_vad_chunk(String::new(), kind, config, session_id, pcm, reset)
                .await;
            drop(permit);
            result.map_err(map_host_error)
        });
        match outcome {
            Ok(event) => Ok(map_event(event)),
            Err(e) => Err(e),
        }
    }
}

impl Drop for IpcVadEngine {
    fn drop(&mut self) {
        // Best-effort teardown: tell the plugin to discard this session's
        // engine state so repeated mic toggles do not leak ONNX sessions in
        // the plugin process. Fire-and-forget on the captured runtime; if it
        // is already shut down the spawn fails silently (the plugin is
        // shutting down too).
        let conn = Arc::clone(&self.conn);
        let kind = self.kind.clone();
        let config = self.current_provider_config();
        let session_id = self.session_id.clone();
        drop(self.handle.spawn(async move {
            drop(
                conn.process_vad_chunk(String::new(), kind, config, session_id, Vec::new(), true)
                    .await,
            );
        }));
    }
}

/// Maps a [`PluginHostError`] into the [`AudioProviderError`] domain.
///
/// Mirrors the STT/TTS adapters: no retries for transport failures (the
/// capture loop escalates after repeated failures and resets the engine).
fn map_host_error(e: crate::error::PluginHostError) -> AudioProviderError {
    match e {
        crate::error::PluginHostError::TransportFailed { message } => {
            AudioProviderError::Provider(format!("plugin VAD transport failed: {message}"))
        }
        crate::error::PluginHostError::ExecutionFailed { message }
            if message.contains("timed out") =>
        {
            AudioProviderError::Timeout
        }
        other => AudioProviderError::Provider(other.to_string()),
    }
}

impl VadEngine for IpcVadEngine {
    fn frame_size(&self) -> usize {
        self.frame_size
    }

    fn process_chunk(&mut self, pcm: &[f32]) -> Result<VadEvent, AudioProviderError> {
        if pcm.len() != self.frame_size {
            return Err(AudioProviderError::Provider(format!(
                "plugin VAD engine expects {} samples per chunk, got {}",
                self.frame_size,
                pcm.len()
            )));
        }
        self.round_trip(pcm.to_vec(), false)
    }

    fn reset(&mut self) {
        // Best-effort: a transport failure here leaves stale state in the
        // plugin, which the next `process_chunk` after a failed `reset`
        // would otherwise continue. The capture loop resets only after
        // repeated failures, so a lost reset is recovered by the same
        // escalation path.
        if let Err(e) = self.round_trip(Vec::new(), true) {
            tracing::warn!(
                component = "PluginHostManager",
                error = %e,
                "VAD reset over IPC failed; plugin session state may be stale"
            );
        }
    }

    fn name(&self) -> &str {
        &self.kind
    }
}

/// Factory that creates [`IpcVadEngine`] instances for a specific engine
/// kind served by a plugin binary.
pub struct IpcVadFactory {
    kind: String,
    conn: Arc<IpcPluginConnection>,
    plugin_name: String,
    frame_size: usize,
    sample_rate: u32,
    handle: tokio::runtime::Handle,
    /// Shared across every engine instance this factory creates, enforcing
    /// the plugin's declared [`ConcurrencyHint`] across sessions.
    limiter: Arc<ConcurrencyLimiter>,
}

impl IpcVadFactory {
    /// Creates a new factory for the given engine kind, sharing the plugin
    /// connection.
    ///
    /// `frame_size` and `sample_rate` come from the plugin's
    /// `VadProviderSpec`; `concurrency` is its declared hint; `handle` is
    /// the runtime that owns the connection (captured at manager startup),
    /// used by each engine to bridge its synchronous `process_chunk` calls.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        frame_size: usize,
        sample_rate: u32,
        concurrency: ConcurrencyHint,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            frame_size,
            sample_rate,
            handle,
            limiter: Arc::new(ConcurrencyLimiter::new(concurrency)),
        }
    }
}

impl VadFactory for IpcVadFactory {
    fn provider_name(&self) -> &str {
        &self.kind
    }

    fn create_engine(
        &self,
        config: &ene_config::EneConfig,
    ) -> Result<Box<dyn VadEngine>, AudioProviderError> {
        let blob = ene_ai::plugin_config::plugin_config_blob(config, &self.plugin_name)
            .unwrap_or_default();
        Ok(Box::new(IpcVadEngine::new(
            self.kind.clone(),
            Arc::clone(&self.conn),
            self.plugin_name.clone(),
            self.frame_size,
            self.sample_rate,
            blob,
            self.handle.clone(),
            Arc::clone(&self.limiter),
        )))
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
        ConcurrencyHint, IpcListener, PluginCapabilities, PluginIpcRequest, PluginIpcResponse,
        WireFormat, cleanup_path, read_plugin_request, write_plugin_response,
    };
    use tokio::sync::Mutex;

    use super::*;

    /// A scripted fake VAD plugin: completes the handshake at `ack_version`,
    /// then answers `ProcessVadChunk` with `SpeechStart` (or `Silence` for
    /// resets). `received` records every `(session_id, reset)` pair so tests
    /// can assert teardown behavior.
    async fn run_mock_vad_server(
        socket_path: PathBuf,
        ack_version: u32,
        received: Arc<Mutex<Vec<(String, bool)>>>,
    ) {
        cleanup_path(&socket_path);
        let Ok(mut listener) = IpcListener::bind(&socket_path) else {
            return;
        };
        loop {
            let Ok(stream) = listener.accept().await else {
                break;
            };
            let received = Arc::clone(&received);
            tokio::spawn(async move {
                let (mut read_half, write_half) = tokio::io::split(stream);
                let writer = Arc::new(Mutex::new(write_half));
                let mut format = WireFormat::Json;
                while let Ok(Some(req)) = read_plugin_request(&mut read_half, format).await {
                    let resp_format = if matches!(&req, PluginIpcRequest::Handshake { .. }) {
                        format = WireFormat::for_version(ack_version);
                        WireFormat::Json
                    } else {
                        format
                    };
                    let resp = match req {
                        PluginIpcRequest::Handshake { .. } => PluginIpcResponse::HandshakeAck {
                            version: ack_version,
                            capabilities: PluginCapabilities {
                                tools: 0,
                                llm_providers: Vec::new(),
                                tts_providers: Vec::new(),
                                stt_providers: Vec::new(),
                                ..PluginCapabilities::default()
                            },
                        },
                        PluginIpcRequest::ProcessVadChunk {
                            request_id,
                            reset,
                            session_id,
                            ..
                        } => {
                            received.lock().await.push((session_id, reset));
                            PluginIpcResponse::VadChunkResult {
                                request_id,
                                event: if reset {
                                    WireVadEvent::Silence
                                } else {
                                    WireVadEvent::SpeechStart
                                },
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

    async fn spawn_engine(
        ack_version: u32,
        received: Arc<Mutex<Vec<(String, bool)>>>,
    ) -> (IpcVadEngine, tokio::task::JoinHandle<()>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("vad.sock");
        let server = tokio::spawn(run_mock_vad_server(
            socket_path.clone(),
            ack_version,
            received,
        ));
        let conn = crate::ipc_plugin::IpcPluginConnection::connect(
            &socket_path,
            ene_plugin_proto::SandboxConfigData::default(),
            None,
            None,
            std::time::Duration::from_secs(5),
            4,
        )
        .await
        .expect("connect to mock VAD plugin");
        let engine = IpcVadEngine::new(
            "silero".into(),
            Arc::new(conn),
            "onnx".into(),
            512,
            16_000,
            serde_json::Value::Null,
            tokio::runtime::Handle::current(),
            Arc::new(ConcurrencyLimiter::new(ConcurrencyHint::default())),
        );
        (engine, server, dir)
    }

    // A multi-thread runtime so the OS thread's `block_on` can schedule the
    // connection work while the test thread waits on the channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn process_chunk_roundtrips_event_and_reset() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let (engine, server, _dir) = spawn_engine(7, received).await;
        assert_eq!(engine.frame_size(), 512);
        assert_eq!(engine.sample_rate(), 16_000);
        assert_eq!(engine.name(), "silero");
        // `process_chunk` blocks on the runtime handle, which tokio forbids
        // from a runtime thread (the production caller is the capture
        // thread); drive it from a plain OS thread like the real caller.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = engine;
        std::thread::spawn(move || {
            let event = engine.process_chunk(&vec![0.0; 512]);
            engine.reset();
            drop(tx.send(event));
        });
        assert_eq!(
            rx.recv().expect("process result").expect("process ok"),
            VadEvent::SpeechStart
        );
        server.abort();
    }

    /// The negotiated-version gate: `ProcessVadChunk` exists since v7, so
    /// every version the host still negotiates (v7 = N-1, v8 = current)
    /// must report VAD support.
    #[tokio::test(flavor = "multi_thread")]
    async fn supports_vad_follows_negotiated_version() {
        for (ack_version, expected) in [(7, true), (8, true)] {
            let received = Arc::new(Mutex::new(Vec::new()));
            let dir = tempfile::tempdir().expect("tempdir");
            let socket_path = dir.path().join(format!("vad-{ack_version}.sock"));
            let server = tokio::spawn(run_mock_vad_server(
                socket_path.clone(),
                ack_version,
                received,
            ));
            let conn = crate::ipc_plugin::IpcPluginConnection::connect(
                &socket_path,
                ene_plugin_proto::SandboxConfigData::default(),
                None,
                None,
                std::time::Duration::from_secs(5),
                4,
            )
            .await
            .expect("connect to mock VAD plugin");
            assert_eq!(conn.supports_vad(), expected);
            server.abort();
        }
    }

    /// Dropping the engine must send a session teardown (`reset`) so
    /// repeated mic toggles do not leak ONNX sessions in the plugin process.
    #[tokio::test(flavor = "multi_thread")]
    async fn drop_sends_session_teardown() {
        let received = Arc::new(Mutex::new(Vec::new()));
        let (engine, server, _dir) = spawn_engine(7, Arc::clone(&received)).await;
        let session_id = engine.session_id.clone();
        drop(engine);

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = received.lock().await.clone();
            if seen
                .iter()
                .any(|(session, reset)| session == &session_id && *reset)
            {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for session teardown"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        server.abort();
    }

    #[test]
    fn wire_event_mapping_covers_all_host_events() {
        for (wire, host) in [
            (WireVadEvent::SpeechStart, VadEvent::SpeechStart),
            (WireVadEvent::SpeechContinue, VadEvent::SpeechContinue),
            (WireVadEvent::SpeechEnd, VadEvent::SpeechEnd),
            (WireVadEvent::Silence, VadEvent::Silence),
        ] {
            assert_eq!(map_event(wire), host);
        }
    }

    #[test]
    fn session_ids_are_unique() {
        assert_ne!(next_session_id(), next_session_id());
    }
}
