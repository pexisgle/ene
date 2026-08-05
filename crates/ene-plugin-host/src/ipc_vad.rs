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
//! the plugin-side state. Sessions are inherently serial (one capture loop
//! per engine), so no [`ConcurrencyLimiter`](crate::ipc_provider::ConcurrencyLimiter)
//! gates the calls — the plugin's own mutex serializes any concurrent
//! sessions.

use std::sync::Arc;

use ene_ai::AudioProviderError;
use ene_ai::traits::VadEngine;
use ene_ai::{VadEvent, VadFactory};
use ene_plugin_proto::VadEvent as WireVadEvent;

use crate::ipc_plugin::IpcPluginConnection;

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
    config_snapshot: serde_json::Value,
    handle: tokio::runtime::Handle,
}

impl IpcVadEngine {
    /// Creates a new IPC-backed VAD engine with a fresh session id.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        frame_size: usize,
        config_snapshot: serde_json::Value,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            session_id: next_session_id(),
            frame_size,
            config_snapshot,
            handle,
        }
    }

    /// Returns the live plugin blob (or the creation-time snapshot).
    fn current_provider_config(&self) -> serde_json::Value {
        ene_ai::plugin_config::global_plugin_config_blob(&self.plugin_name)
            .unwrap_or_else(|| self.config_snapshot.clone())
    }

    fn round_trip(&self, pcm: Vec<f32>, reset: bool) -> Result<VadEvent, AudioProviderError> {
        let conn = Arc::clone(&self.conn);
        let kind = self.kind.clone();
        let config = self.current_provider_config();
        let session_id = self.session_id.clone();
        let outcome = self.handle.block_on(async move {
            conn.process_vad_chunk(String::new(), kind, config, session_id, pcm, reset)
                .await
        });
        match outcome {
            Ok(event) => Ok(map_event(event)),
            Err(e) => Err(map_host_error(e)),
        }
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
    handle: tokio::runtime::Handle,
}

impl IpcVadFactory {
    /// Creates a new factory for the given engine kind, sharing the plugin
    /// connection.
    ///
    /// `frame_size` comes from the plugin's `VadProviderSpec`; `handle` is
    /// the runtime that owns the connection (captured at manager startup),
    /// used by each engine to bridge its synchronous `process_chunk` calls.
    #[must_use]
    pub fn new(
        kind: String,
        conn: Arc<IpcPluginConnection>,
        plugin_name: String,
        frame_size: usize,
        handle: tokio::runtime::Handle,
    ) -> Self {
        Self {
            kind,
            conn,
            plugin_name,
            frame_size,
            handle,
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
            blob,
            self.handle.clone(),
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
        IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
        PluginIpcResponse, WireFormat, cleanup_path, read_plugin_request, write_plugin_response,
    };
    use tokio::sync::Mutex;

    use super::*;

    /// A scripted fake VAD plugin: completes the handshake, then answers
    /// `ProcessVadChunk` with `SpeechStart` (or `Silence` for resets).
    async fn run_mock_vad_server(socket_path: PathBuf) {
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
                        PluginIpcRequest::ProcessVadChunk {
                            request_id, reset, ..
                        } => PluginIpcResponse::VadChunkResult {
                            request_id,
                            event: if reset {
                                WireVadEvent::Silence
                            } else {
                                WireVadEvent::SpeechStart
                            },
                        },
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

    // A multi-thread runtime so the OS thread's `block_on` can schedule the
    // connection work while the test thread waits on the channel.
    #[tokio::test(flavor = "multi_thread")]
    async fn process_chunk_roundtrips_event_and_reset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket_path = dir.path().join("vad.sock");
        let server = tokio::spawn(run_mock_vad_server(socket_path.clone()));
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
        let factory = IpcVadFactory::new(
            "silero".into(),
            Arc::new(conn),
            "onnx".into(),
            512,
            tokio::runtime::Handle::current(),
        );
        let engine = factory
            .create_engine(&ene_config::EneConfig::default())
            .expect("create engine");
        assert_eq!(engine.frame_size(), 512);
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
