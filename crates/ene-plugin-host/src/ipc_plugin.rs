//! IPC client to a single plugin binary.
//!
//! [`IpcPluginConnection`] manages the lifecycle of one connection to a
//! plugin process: handshake, request/response, and reconnection on failure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use ene_plugin_proto::{
    CallContext, ConfigFieldError, ConfigOption, DeferredOutcome, DeferredStatus, IpcStream,
    PluginCapabilities, PluginIpcRequest, PluginIpcResponse, SandboxConfigData, ToolError,
    ToolResult, VersionRange, WireFormat, read_plugin_response, write_plugin_request,
};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::PluginHostError;

/// Maximum number of connection retries with backoff.
const CONNECT_MAX_RETRIES: u32 = 50;
/// Delay between connection retry attempts.
const CONNECT_DELAY: Duration = Duration::from_millis(50);
/// Default per-call timeout (2 min — LLM calls can be slow).
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(2);
/// Timeout for a `Ping` liveness probe.
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// Generates a unique request identifier for IPC request/response correlation.
fn next_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Protocol version at which [`PluginIpcRequest::CancelStream`] was
/// introduced (see the `PLUGIN_IPC_PROTOCOL_VERSION` docs in
/// `ene-plugin-proto`). Plugins that negotiated an older version do not know
/// this message variant; sending it to them would fail to deserialize on
/// their end.
const CANCEL_STREAM_MIN_VERSION: u32 = 4;

/// Protocol version at which [`PluginIpcRequest::SetConfig`] was introduced.
/// Plugins that negotiated an older version do not know this message
/// variant; the host updates its local cache and skips the IPC send.
const SET_CONFIG_MIN_VERSION: u32 = 5;

/// How a [`IpcPluginConnection::set_config`] call delivers the update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetConfigOutcome {
    /// `SetConfig` IPC was delivered to the live plugin.
    Pushed,
    /// The peer negotiated a protocol below `SetConfig` support; only the
    /// reconnect cache was updated.
    CachedOnly,
}

/// Protocol version at which the dynamic-config message family was introduced
/// (same generation as [`SET_CONFIG_MIN_VERSION`]). Hosts still require the
/// matching [`PluginCapabilities`] flags before sending, so older v5 binaries
/// that lack the variants are never addressed.
const DYNAMIC_CONFIG_MIN_VERSION: u32 = 5;

/// Protocol version at which [`PluginIpcRequest::ProcessVadChunk`] was
/// introduced (IPC v7). Plugins that negotiated an older version do not know
/// this message variant; sending it to them would fail to deserialize on
/// their end.
const PROCESS_VAD_CHUNK_MIN_VERSION: u32 = 7;

/// Shared routing state for the single reader task.
///
/// Every incoming [`PluginIpcResponse`] is dispatched here by the reader task
/// *before* any request/response correlation, so push messages
/// ([`DeferredCompleted`](PluginIpcResponse::DeferredCompleted)) and stream
/// messages can never be stolen by an unrelated in-flight request.
///
/// All internal locks are `parking_lot::Mutex` because they are held only for
/// the duration of a map lookup/insert — never across an `.await`.
struct Router {
    /// Per-request oneshot waiters for request/response correlation.
    waiters: parking_lot::Mutex<HashMap<String, oneshot::Sender<PluginIpcResponse>>>,
    /// Per-request stream channels for chat streams, keyed by `request_id`.
    streams: parking_lot::Mutex<HashMap<String, mpsc::Sender<PluginIpcResponse>>>,
    /// Push cache for deferred task completions, keyed by `task_id`.
    ///
    /// Populated by the reader task when a `DeferredCompleted` push arrives;
    /// drained by [`IpcPluginConnection::poll_deferred`] so a completion that
    /// arrived while the connection was idle is still delivered.
    deferred: parking_lot::Mutex<HashMap<String, Result<ToolResult, ToolError>>>,
    /// Latest `ConfigSchemaChanged` push, if any has arrived since the last
    /// [`IpcPluginConnection::take_config_schema_changed`] call.
    ///
    /// The stored value is the LATEST push, not a history — a second
    /// `ConfigSchemaChanged` before a poll overwrites the first;
    /// `config_version` is the latest value, not a sequence.
    schema_changed: parking_lot::Mutex<Option<(Option<serde_json::Value>, u32)>>,
}

impl Router {
    fn new() -> Self {
        Self {
            waiters: parking_lot::Mutex::new(HashMap::new()),
            streams: parking_lot::Mutex::new(HashMap::new()),
            deferred: parking_lot::Mutex::new(HashMap::new()),
            schema_changed: parking_lot::Mutex::new(None),
        }
    }

    /// Routes a single response to its destination.
    ///
    /// Push and stream messages are matched by variant *first*, so they can
    /// never fall through to request/response correlation.
    fn dispatch(&self, resp: PluginIpcResponse) {
        match &resp {
            // Push: cache the deferred completion for later retrieval.
            PluginIpcResponse::DeferredCompleted { task_id, result } => {
                self.deferred.lock().insert(task_id.clone(), result.clone());
            }
            // Push: retain the latest schema change (UI may poll later).
            PluginIpcResponse::ConfigSchemaChanged {
                schema,
                config_version,
            } => {
                *self.schema_changed.lock() = Some((schema.clone(), *config_version));
            }
            // Stream: forward to the per-request stream channel.
            PluginIpcResponse::StreamChunk { request_id, .. }
            | PluginIpcResponse::StreamEnd { request_id }
            | PluginIpcResponse::StreamError { request_id, .. } => {
                if let Some(tx) = self.streams.lock().get(request_id) {
                    drop(tx.try_send(resp));
                }
            }
            // Request/response: correlate by `request_id`.
            _ => {
                let rid = response_request_id(&resp).unwrap_or_default();
                if let Some(tx) = self.waiters.lock().remove(rid) {
                    drop(tx.send(resp));
                }
            }
        }
    }

    /// Fails all pending waiters and closes all stream channels.
    ///
    /// Called when the reader task exits (EOF, read error, or reconnect) so
    /// that every in-flight `do_request` / stream reader observes the failure
    /// promptly instead of hanging until its own timeout.
    fn fail_all(&self) {
        self.waiters.lock().clear();
        self.streams.lock().clear();
    }
}

/// Extracts the `request_id` from a response variant, if it carries one.
///
/// Push messages ([`DeferredCompleted`](PluginIpcResponse::DeferredCompleted),
/// [`ConfigSchemaChanged`](PluginIpcResponse::ConfigSchemaChanged)) and
/// [`HandshakeAck`](PluginIpcResponse::HandshakeAck) carry no `request_id`
/// and return `None`.
fn response_request_id(resp: &PluginIpcResponse) -> Option<&str> {
    match resp {
        PluginIpcResponse::HandshakeAck { .. }
        | PluginIpcResponse::DeferredCompleted { .. }
        | PluginIpcResponse::ConfigSchemaChanged { .. } => None,
        PluginIpcResponse::Ack { request_id }
        | PluginIpcResponse::Pong { request_id }
        | PluginIpcResponse::ConfigSchema { request_id, .. }
        | PluginIpcResponse::ConfigApplied { request_id }
        | PluginIpcResponse::ConfigOptions { request_id, .. }
        | PluginIpcResponse::ConfigValidated { request_id, .. }
        | PluginIpcResponse::ConfigMigrated { request_id, .. }
        | PluginIpcResponse::Error { request_id, .. }
        | PluginIpcResponse::Tools { request_id, .. }
        | PluginIpcResponse::CallResult { request_id, .. }
        | PluginIpcResponse::DeferredAccepted { request_id, .. }
        | PluginIpcResponse::DeferredStatus { request_id, .. }
        | PluginIpcResponse::StreamChunk { request_id, .. }
        | PluginIpcResponse::StreamEnd { request_id }
        | PluginIpcResponse::StreamError { request_id, .. }
        | PluginIpcResponse::ChatCompletionResult { request_id, .. }
        | PluginIpcResponse::EmbedBatchResult { request_id, .. }
        | PluginIpcResponse::SpeechResult { request_id, .. }
        | PluginIpcResponse::TranscriptionResult { request_id, .. }
        | PluginIpcResponse::VadChunkResult { request_id, .. } => Some(request_id.as_str()),
    }
}

/// The single reader task for a connection.
///
/// Reads responses from the socket in a loop and dispatches each one through
/// the [`Router`]. Exits on EOF or read error, then fails all pending waiters
/// so in-flight callers observe the transport failure promptly.
async fn reader_loop(mut reader: ReadHalf<IpcStream>, router: Arc<Router>, format: WireFormat) {
    loop {
        match read_plugin_response(&mut reader, format).await {
            Ok(Some(resp)) => router.dispatch(resp),
            Ok(None) => {
                tracing::debug!(
                    component = "IpcPluginConnection",
                    "reader: connection closed (EOF)"
                );
                break;
            }
            Err(e) => {
                tracing::debug!(
                    component = "IpcPluginConnection",
                    error = %e,
                    "reader: read error"
                );
                break;
            }
        }
    }
    router.fail_all();
}

/// An IPC connection to a single plugin binary.
///
/// Handles the handshake, request/response round-trips, and transparent
/// reconnection on transport failure. All request-path methods take `&self`
/// and may be called concurrently from multiple tasks: the socket write half
/// is guarded by its own short-lived [`Mutex`] (released *before* awaiting the
/// response), and responses are correlated by `request_id` through the shared
/// [`Router`]. Callers therefore share a plain `Arc<IpcPluginConnection>` and
/// need no external lock to multiplex requests.
///
/// ## Request multiplexing
///
/// [`request_once`](Self::request_once) registers a oneshot waiter and writes
/// the request atomically under the brief writer lock, releases that lock, and
/// *then* awaits the response. Multiple requests can thus be in flight at once
/// against the same connection — the single reader task routes each response
/// to its waiter by `request_id`. A connection-level [`Semaphore`] bounds the
/// number of concurrent in-flight requests (see [`connect`](Self::connect)).
///
/// ## Single reader task
///
/// A dedicated reader task reads all responses from the socket and dispatches
/// them through a [`Router`] that routes by `request_id`/variant *before*
/// request/response correlation. This eliminates the "between-reads lock
/// release" pattern and ensures push messages (`DeferredCompleted`) and stream
/// messages are never stolen by an unrelated in-flight request.
pub struct IpcPluginConnection {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    /// Plugin configuration re-sent on every (re)connect handshake and
    /// updated by [`set_config`](Self::set_config) before a live `SetConfig`
    /// IPC push so reconnect always uses the freshest value.
    plugin_config: parking_lot::RwLock<Option<serde_json::Value>>,
    /// Per-profile plugin configuration (`plugins.list.<name>.profiles`),
    /// re-sent to the plugin on every (re)connect handshake alongside
    /// [`plugin_config`](Self::plugin_config).
    plugin_profiles: parking_lot::RwLock<Option<serde_json::Value>>,
    /// Write half of the IPC stream, behind its own lock so the write is
    /// serialized (frames never interleave) but released before the response
    /// wait. The read half is owned by the reader task. `None` while
    /// [`reconnect`](Self::reconnect) is swapping the stream, and it *stays*
    /// `None` if a reconnect attempt fails (the stale stream is already gone);
    /// every request then fails with "not connected" until a later reconnect
    /// succeeds.
    writer: Mutex<Option<WriteHalf<IpcStream>>>,
    capabilities: parking_lot::RwLock<PluginCapabilities>,
    /// The protocol version negotiated with the plugin during the handshake
    /// (see [`VersionRange::negotiate`]). Used to gate host behavior that
    /// depends on a message variant introduced after v3 — see
    /// [`supports_cancel_stream`](Self::supports_cancel_stream) for the
    /// documented pattern to follow when adding another gate.
    negotiated_version: AtomicU32,
    timeout: Duration,
    /// Timeout applied to the handshake response read. Captured at
    /// [`connect`](Self::connect) time so [`reconnect`](Self::reconnect)
    /// reuses the same bound without re-reading configuration.
    handshake_timeout: Duration,
    /// Shared routing state for the reader task.
    router: Arc<Router>,
    /// Handle to the reader task, behind a lock so [`reconnect`](Self::reconnect)
    /// can abort and replace it through `&self`. Aborted on reconnect/shutdown.
    reader_task: Mutex<Option<JoinHandle<()>>>,
    /// Monotonic connection generation, incremented on every successful
    /// (re)connect. Lets the request path coalesce concurrent reconnects: when
    /// a shared transport failure fails many in-flight requests at once, only
    /// the first one actually reconnects and the rest observe the advanced
    /// generation and simply retry on the fresh connection. Without
    /// this, each failed request would tear down the connection its sibling
    /// just re-established.
    ///
    /// The coalescing check is performed *under the writer lock* in
    /// [`reconnect_from`](Self::reconnect_from): a caller snapshots the
    /// generation before its request, and `reconnect_from` re-reads it after
    /// acquiring the lock, so two siblings that both fail before either
    /// reconnects cannot both tear the connection down — the second observes
    /// the advanced generation and returns without reconnecting.
    generation: AtomicU64,
    /// Bounds the number of concurrent in-flight requests against this
    /// connection. A permit is acquired in
    /// [`request_once`](Self::request_once) and held for the round-trip, so the
    /// plugin is protected from unbounded host fan-out. Sized from
    /// `PluginConfig::max_concurrent` at [`connect`](Self::connect) time.
    inflight: Arc<Semaphore>,
}

impl IpcPluginConnection {
    /// Connects to a plugin binary at `socket_path`, performs the protocol
    /// handshake (advertising the host's supported version range), and stores
    /// the advertised capabilities.
    ///
    /// `max_concurrent` bounds the number of concurrent in-flight requests
    /// against this connection; it is clamped to the semaphore's
    /// valid range (`1..=Semaphore::MAX_PERMITS`) and is normally sourced from
    /// `PluginConfig::max_concurrent`.
    ///
    /// Retries the connect up to [`CONNECT_MAX_RETRIES`] times with a
    /// fixed delay, giving the child process time to bind its listener.
    ///
    /// The handshake response is awaited for at most `handshake_timeout`;
    /// a plugin that accepts the socket but never replies fails fast with
    /// [`PluginHostError::HandshakeFailed`] instead of blocking startup
    /// indefinitely. Plugins that perform heavy initialization should
    /// respond to the handshake promptly and defer expensive work until
    /// afterwards.
    pub async fn connect(
        socket_path: &Path,
        sandbox: SandboxConfigData,
        plugin_config: Option<serde_json::Value>,
        plugin_profiles: Option<serde_json::Value>,
        handshake_timeout: Duration,
        max_concurrent: usize,
    ) -> Result<Self, PluginHostError> {
        let name = socket_path.file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        let mut stream = Self::connect_with_retry(socket_path, &name).await?;

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange::host_supported(),
                sandbox: sandbox.clone(),
                plugin_config: plugin_config.clone(),
                plugin_profiles: plugin_profiles.clone(),
            },
            WireFormat::Json,
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake: {e}"),
        })?;

        let resp = tokio::time::timeout(
            handshake_timeout,
            read_plugin_response(&mut stream, WireFormat::Json),
        )
        .await
        .map_err(|_| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!(
                "no HandshakeAck within {} ms (plugin accepted the socket but never \
                         responded; defer heavy initialization until after the handshake)",
                handshake_timeout.as_millis()
            ),
        })?
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to read HandshakeAck: {e}"),
        })?;

        let (negotiated_version, capabilities) = match resp {
            Some(PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            }) => {
                // The plugin picks the highest mutually-supported version, so
                // accept any negotiated version inside our advertised range
                // rather than requiring an exact match.
                let host_range = VersionRange::host_supported();
                if !host_range.contains(version) {
                    return Err(PluginHostError::ProtocolMismatch {
                        name,
                        host_min: host_range.min,
                        host_max: host_range.max,
                        got: version,
                    });
                }
                (version, capabilities)
            }
            Some(PluginIpcResponse::Error { message, .. }) => {
                // The plugin's version ranges did not overlap with the
                // host's; the plugin's diagnostic message already includes
                // both ranges (see `dispatch_request` in `ene-plugin`'s
                // `server.rs`), so it is preserved verbatim here.
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: format!(
                        "{message} (host supports protocol {}..={})",
                        VersionRange::host_supported().min,
                        VersionRange::host_supported().max
                    ),
                });
            }
            _ => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: "unexpected response to Handshake".to_string(),
                });
            }
        };

        let router = Arc::new(Router::new());
        let (reader, writer) = tokio::io::split(stream);
        let reader_task = tokio::spawn(reader_loop(
            reader,
            Arc::clone(&router),
            WireFormat::for_version(negotiated_version),
        ));

        Ok(Self {
            socket_path: socket_path.to_path_buf(),
            sandbox,
            plugin_config: parking_lot::RwLock::new(plugin_config),
            plugin_profiles: parking_lot::RwLock::new(plugin_profiles),
            writer: Mutex::new(Some(writer)),
            capabilities: parking_lot::RwLock::new(capabilities),
            negotiated_version: AtomicU32::new(negotiated_version),
            timeout: DEFAULT_TIMEOUT,
            handshake_timeout,
            router,
            reader_task: Mutex::new(Some(reader_task)),
            // The initial connection is generation 1; each successful
            // `reconnect` advances it (see `reconnect`).
            generation: AtomicU64::new(1),
            // `Semaphore::new` panics above `Semaphore::MAX_PERMITS`, and the
            // size is config-driven (`plugins.max_concurrent`); clamp to the
            // valid range so a misconfiguration degrades gracefully instead of
            // panicking the host (workspace no-panic contract).
            inflight: Arc::new(Semaphore::new(
                max_concurrent.clamp(1, Semaphore::MAX_PERMITS),
            )),
        })
    }

    /// Returns the capabilities advertised by the plugin during the handshake.
    pub fn capabilities(&self) -> PluginCapabilities {
        self.capabilities.read().clone()
    }

    /// Returns the protocol version negotiated with the plugin during the
    /// handshake.
    ///
    /// Falls within [`VersionRange::host_supported`] (currently
    /// `PLUGIN_IPC_MIN_SUPPORTED_VERSION..=PLUGIN_IPC_PROTOCOL_VERSION`).
    /// Feature gates that depend on a message variant introduced in a later
    /// protocol version should compare against this value — see
    /// [`supports_cancel_stream`](Self::supports_cancel_stream) for the
    /// pattern to follow.
    pub fn negotiated_version(&self) -> u32 {
        self.negotiated_version.load(Ordering::Acquire)
    }

    /// Returns the payload framing negotiated with the plugin.
    ///
    /// The handshake exchange always uses JSON; every later frame uses the
    /// format negotiated for the agreed protocol version.
    fn wire_format(&self) -> WireFormat {
        WireFormat::for_version(self.negotiated_version())
    }

    /// Returns whether the negotiated protocol version supports explicit
    /// stream cancellation via [`PluginIpcRequest::CancelStream`].
    ///
    /// `CancelStream` was introduced in protocol v4 (see the
    /// `PLUGIN_IPC_PROTOCOL_VERSION` docs in `ene-plugin-proto`). Every peer
    /// in the host's N-1 window (v5+) knows this variant, so
    /// [`cancel_stream`](Self::cancel_stream) always sends it; the check is
    /// retained as the version-relative pattern that any feature introduced
    /// above the minimum must follow.
    ///
    /// This is the pattern to follow for any future version-gated feature:
    /// add a `const fn supports_x(&self) -> bool` here that compares
    /// `self.negotiated_version` against the version the feature was
    /// introduced in, and branch on it wherever the feature is used.
    pub fn supports_cancel_stream(&self) -> bool {
        self.negotiated_version() >= CANCEL_STREAM_MIN_VERSION
    }

    /// Returns whether the negotiated protocol version supports live config
    /// updates via [`PluginIpcRequest::SetConfig`].
    ///
    /// `SetConfig` was introduced in protocol v5. Every peer in the host's
    /// N-1 window (v5+) knows this variant, so
    /// [`set_config`](Self::set_config) always sends the live IPC push.
    pub fn supports_set_config(&self) -> bool {
        self.negotiated_version() >= SET_CONFIG_MIN_VERSION
    }

    /// Returns whether the peer can handle [`PluginIpcRequest::ListConfigOptions`].
    ///
    /// Requires protocol ≥ v5 **and**
    /// [`PluginCapabilities::supports_list_config_options`]. Older v5 binaries
    /// that omit the flag (serde default `false`) are never sent the variant.
    pub fn supports_list_config_options(&self) -> bool {
        self.negotiated_version() >= DYNAMIC_CONFIG_MIN_VERSION
            && self.capabilities().supports_list_config_options
    }

    /// Returns whether the peer can handle [`PluginIpcRequest::ValidateConfig`].
    pub fn supports_validate_config(&self) -> bool {
        self.negotiated_version() >= DYNAMIC_CONFIG_MIN_VERSION
            && self.capabilities().supports_validate_config
    }

    /// Returns whether the peer can handle [`PluginIpcRequest::MigrateConfig`].
    pub fn supports_migrate_config(&self) -> bool {
        self.negotiated_version() >= DYNAMIC_CONFIG_MIN_VERSION
            && self.capabilities().supports_migrate_config
    }

    /// Returns whether the peer can handle
    /// [`PluginIpcRequest::ProcessVadChunk`].
    ///
    /// `ProcessVadChunk` was introduced in protocol v7, so only peers that
    /// negotiated v7+ receive VAD requests. The host additionally only
    /// registers a VAD factory when the handshake advertised
    /// `vad_providers`, so a v7 peer without VAD never gets one.
    pub fn supports_vad(&self) -> bool {
        self.negotiated_version() >= PROCESS_VAD_CHUNK_MIN_VERSION
    }

    /// Sends a `ListTools` request and returns the actual tool specs.
    pub async fn list_tools(&self) -> Result<Vec<ene_plugin_proto::ToolSpec>, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::ListTools {
                request_id: String::new(),
            })
            .await?;
        match resp {
            PluginIpcResponse::Tools { tools, .. } => Ok(tools),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ListTools: {other:?}"
            ))),
        }
    }

    /// Requests the plugin's config JSON Schema for schema-aware redaction of
    /// host log output.
    ///
    /// Safe to call repeatedly (runtime re-fetch). Returns `None` when the
    /// plugin advertises no schema, or sends `null` or an empty object;
    /// callers then fall back to the schema-independent redaction
    /// ([`crate::redact::redact_config_unschematized`]).
    pub async fn config_schema(&self) -> Result<Option<serde_json::Value>, PluginHostError> {
        Ok(self.config_schema_with_version().await?.0)
    }

    /// Like [`config_schema`](Self::config_schema), but also returns the
    /// plugin's current `config_version` (0 when unversioned / omitted).
    pub async fn config_schema_with_version(
        &self,
    ) -> Result<(Option<serde_json::Value>, u32), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::GetConfigSchema {
                request_id: String::new(),
            })
            .await?;
        match resp {
            PluginIpcResponse::ConfigSchema {
                schema,
                config_version,
                ..
            } => Ok((
                schema.filter(|s| {
                    !s.is_null() && !matches!(s, serde_json::Value::Object(o) if o.is_empty())
                }),
                config_version,
            )),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to GetConfigSchema: {other:?}"
            ))),
        }
    }

    /// Lists dynamic options for a config path, or an empty list when the
    /// peer does not advertise [`supports_list_config_options`](Self::supports_list_config_options)
    /// (static-schema degrade path).
    pub async fn list_config_options(
        &self,
        path: &str,
    ) -> Result<Vec<ConfigOption>, PluginHostError> {
        if !self.supports_list_config_options() {
            tracing::debug!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version(),
                path,
                "plugin does not support ListConfigOptions; returning empty options"
            );
            return Ok(Vec::new());
        }
        let resp = self
            .do_request(PluginIpcRequest::ListConfigOptions {
                request_id: String::new(),
                path: path.to_string(),
            })
            .await?;
        match resp {
            PluginIpcResponse::ConfigOptions { options, .. } => Ok(options),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ListConfigOptions: {other:?}"
            ))),
        }
    }

    /// Validates a config value via the plugin, or returns `Ok(vec![])`
    /// (no plugin-side errors) when the peer does not advertise
    /// [`supports_validate_config`](Self::supports_validate_config).
    ///
    /// Callers that need schema validation for unsupported peers should run
    /// host-side JSON Schema validation themselves.
    pub async fn validate_config(
        &self,
        value: &serde_json::Value,
    ) -> Result<Vec<ConfigFieldError>, PluginHostError> {
        if !self.supports_validate_config() {
            tracing::debug!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version(),
                "plugin does not support ValidateConfig; host should use JSON Schema"
            );
            return Ok(Vec::new());
        }
        let resp = self
            .do_request(PluginIpcRequest::ValidateConfig {
                request_id: String::new(),
                value: value.clone(),
            })
            .await?;
        match resp {
            PluginIpcResponse::ConfigValidated { errors, .. } => Ok(errors),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ValidateConfig: {other:?}"
            ))),
        }
    }

    /// Migrates a stored config blob, or returns the input unchanged when the
    /// peer does not advertise [`supports_migrate_config`](Self::supports_migrate_config).
    ///
    /// The returned tuple is `(migrated_value, config_version)`.
    pub async fn migrate_config(
        &self,
        from_version: u32,
        value: serde_json::Value,
    ) -> Result<(serde_json::Value, u32), PluginHostError> {
        if !self.supports_migrate_config() {
            tracing::debug!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version(),
                from_version,
                "plugin does not support MigrateConfig; returning value unchanged"
            );
            return Ok((value, from_version));
        }
        let resp = self
            .do_request(PluginIpcRequest::MigrateConfig {
                request_id: String::new(),
                from_version,
                value,
            })
            .await?;
        match resp {
            PluginIpcResponse::ConfigMigrated {
                value,
                config_version,
                ..
            } => Ok((value, config_version)),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to MigrateConfig: {other:?}"
            ))),
        }
    }

    /// Takes the latest [`PluginIpcResponse::ConfigSchemaChanged`] push, if
    /// any has arrived since the previous take.
    ///
    /// Returns `(schema, config_version)`. The stored value is the LATEST
    /// push, not a history — a second `ConfigSchemaChanged` before a poll
    /// overwrites the first, so `config_version` is the latest value, not a
    /// sequence. Analogous to the deferred-completion cache used by
    /// [`poll_deferred`](Self::poll_deferred).
    pub fn take_config_schema_changed(&self) -> Option<(Option<serde_json::Value>, u32)> {
        self.router.schema_changed.lock().take()
    }

    /// Sends a `Ping` and waits for `Pong` within [`PING_TIMEOUT`].
    pub async fn ping(&self) -> Result<(), PluginHostError> {
        let resp = self
            .do_request_with_timeout(
                PluginIpcRequest::Ping {
                    request_id: String::new(),
                },
                PING_TIMEOUT,
            )
            .await?;
        match resp {
            PluginIpcResponse::Pong { .. } => Ok(()),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to Ping: {other:?}"
            ))),
        }
    }

    /// Calls a tool exposed by the plugin and returns the result.
    ///
    /// Tool-level failures are propagated as
    /// [`PluginHostError::Protocol`] so callers (e.g. the runtime's
    /// streaming layer) can still match on structured variants such as
    /// `PermissionRequired` and `UserInputRequired`. Flattening the
    /// [`ene_plugin_proto::ToolError`] into a string here would silently
    /// disable the interactive permission / user-input contract.
    ///
    /// When `context` is `Some`, it is included in the `CallTool` IPC
    /// request so the plugin receives it scoped to this single call.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
        context: Option<CallContext>,
    ) -> Result<ToolResult, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::CallTool {
                request_id: String::new(),
                name: name.to_string(),
                arguments: arguments.to_string(),
                deferred: false,
                context,
            })
            .await?;
        match resp {
            PluginIpcResponse::CallResult { result, .. } => {
                result.map_err(PluginHostError::Protocol)
            }
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to CallTool: {other:?}"
            ))),
        }
    }

    /// Sets the call context (conversation + turn identifiers) on the plugin.
    ///
    /// Deprecated: pass context directly via [`call_tool`](Self::call_tool)
    /// instead. The context applies to every subsequent tool call on this
    /// connection; the wire protocol carries only the identifiers, so no
    /// tool name is needed at the connection level (tool routing happens in
    /// the composite registry above).
    pub async fn set_call_context(&self, ctx: &CallContext) -> Result<(), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::SetCallContext {
                request_id: String::new(),
                conversation_id: ctx.conversation_id.clone(),
                turn_id: ctx.turn_id.clone(),
            })
            .await?;
        Self::expect_ack(resp, "SetCallContext")
    }

    /// Approves (or denies) a pending permission request by its identifier.
    pub async fn approve_permission(
        &self,
        permission_request_id: &str,
    ) -> Result<(), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::ApprovePermission {
                request_id: String::new(),
                permission_request_id: permission_request_id.to_string(),
            })
            .await?;
        Self::expect_ack(resp, "ApprovePermission")
    }

    /// Registers a session-wide permission allow pattern (action + target glob).
    pub async fn allow_pattern(
        &self,
        action: &str,
        target_pattern: &str,
    ) -> Result<(), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::AllowPattern {
                request_id: String::new(),
                action: action.to_string(),
                target_pattern: target_pattern.to_string(),
            })
            .await?;
        Self::expect_ack(resp, "AllowPattern")
    }

    /// Revokes a previously granted session-wide permission allow pattern.
    pub async fn revoke_pattern(
        &self,
        action: &str,
        target_pattern: &str,
    ) -> Result<(), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::RevokePattern {
                request_id: String::new(),
                action: action.to_string(),
                target_pattern: target_pattern.to_string(),
            })
            .await?;
        Self::expect_ack(resp, "RevokePattern")
    }

    /// Calls a tool in deferred (background) mode.
    ///
    /// A background-capable tool responds with [`DeferredOutcome::Deferred`]
    /// carrying a `task_id`; any other tool falls back to
    /// [`DeferredOutcome::Sync`] with the ordinary synchronous result.
    ///
    /// When `context` is `Some`, it is included in the `CallTool` IPC
    /// request so the plugin receives it scoped to this single call.
    pub async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
        context: Option<CallContext>,
    ) -> Result<DeferredOutcome, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::CallTool {
                request_id: String::new(),
                name: name.to_string(),
                arguments: arguments.to_string(),
                deferred: true,
                context,
            })
            .await?;
        match resp {
            PluginIpcResponse::CallResult { result, .. } => match result {
                Ok(value) => Ok(DeferredOutcome::Sync(value)),
                Err(e) => Err(PluginHostError::Protocol(e)),
            },
            PluginIpcResponse::DeferredAccepted { task_id, .. } => {
                Ok(DeferredOutcome::Deferred { task_id })
            }
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to deferred CallTool: {other:?}"
            ))),
        }
    }

    /// Polls the status of a deferred (background) task by its identifier.
    ///
    /// Checks the local completion cache first (populated by the reader task
    /// when a `DeferredCompleted` push arrives) before issuing a `PollDeferred`
    /// request. This ensures a completion that arrived while the connection
    /// was idle is delivered promptly.
    pub async fn poll_deferred(&self, task_id: &str) -> Result<DeferredStatus, PluginHostError> {
        if let Some(result) = self.router.deferred.lock().remove(task_id) {
            return Ok(match result {
                Ok(value) => DeferredStatus::Completed { result: value },
                Err(e) => DeferredStatus::Failed {
                    error: e.to_string(),
                },
            });
        }

        let resp = self
            .do_request(PluginIpcRequest::PollDeferred {
                request_id: String::new(),
                task_id: task_id.to_string(),
            })
            .await?;
        match resp {
            PluginIpcResponse::DeferredStatus { status, .. } => Ok(status),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to PollDeferred: {other:?}"
            ))),
        }
    }

    /// Cancels a deferred (background) task by its identifier.
    pub async fn cancel_deferred(&self, task_id: &str) -> Result<(), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::CancelDeferred {
                request_id: String::new(),
                task_id: task_id.to_string(),
            })
            .await?;
        Self::expect_ack(resp, "CancelDeferred")
    }

    /// Cancels an in-progress chat stream by its `request_id`.
    ///
    /// A no-op when the negotiated protocol version does not support
    /// [`PluginIpcRequest::CancelStream`] (see
    /// [`supports_cancel_stream`](Self::supports_cancel_stream)) — the
    /// caller must rely on its existing timeout-based fallback to end the
    /// stream on such plugins rather than requiring the new message.
    pub async fn cancel_stream(&self, request_id: &str) -> Result<(), PluginHostError> {
        if !self.supports_cancel_stream() {
            tracing::debug!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version(),
                request_id,
                "plugin negotiated a version below CancelStream support; \
                 relying on timeout-based fallback instead of sending CancelStream"
            );
            return Ok(());
        }
        let resp = self
            .do_request(PluginIpcRequest::CancelStream {
                request_id: String::new(),
                stream_request_id: request_id.to_string(),
            })
            .await?;
        Self::expect_ack(resp, "CancelStream")
    }

    /// Updates the stored config/profiles and, when supported, pushes
    /// [`PluginIpcRequest::SetConfig`] to the live plugin.
    ///
    /// The local cache is always updated first so a later reconnect
    /// handshake delivers the fresh values even when the peer negotiated a
    /// protocol version below [`supports_set_config`](Self::supports_set_config)
    /// (in that case the IPC send is skipped with a warning and
    /// [`SetConfigOutcome::CachedOnly`] is returned).
    ///
    /// Returns [`SetConfigOutcome::Pushed`] when the live plugin received the
    /// update.
    pub async fn set_config(
        &self,
        config: Option<serde_json::Value>,
        profiles: Option<serde_json::Value>,
    ) -> Result<SetConfigOutcome, PluginHostError> {
        self.plugin_config.write().clone_from(&config);
        self.plugin_profiles.write().clone_from(&profiles);

        if !self.supports_set_config() {
            tracing::warn!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version(),
                "plugin negotiated a version below SetConfig support; \
                 local config cache updated for reconnect, live push skipped"
            );
            return Ok(SetConfigOutcome::CachedOnly);
        }

        let resp = self
            .do_request(PluginIpcRequest::SetConfig {
                request_id: String::new(),
                config: config.unwrap_or_else(|| serde_json::json!({})),
                profiles,
            })
            .await?;
        match resp {
            PluginIpcResponse::ConfigApplied { .. } => Ok(SetConfigOutcome::Pushed),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to SetConfig: {other:?}"
            ))),
        }
    }

    /// Sends a `CreateChatStream` request and returns a receiver for the
    /// stream's responses.
    ///
    /// A per-request channel is registered with the [`Router`] *before* the
    /// request is written, so the single reader task routes every
    /// `StreamChunk` / `StreamEnd` / `StreamError` for this `request_id` into
    /// the returned receiver. The receiver yields responses until a
    /// terminal `StreamEnd`/`StreamError` is observed or the channel closes
    /// (connection failure or stream cancellation).
    pub async fn send_create_chat_stream(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<serde_json::Value>,
        tools: Vec<serde_json::Value>,
    ) -> Result<mpsc::Receiver<PluginIpcResponse>, PluginHostError> {
        let (tx, rx) = mpsc::channel::<PluginIpcResponse>(32);
        self.router.streams.lock().insert(request_id.clone(), tx);

        if let Err(e) = self
            .send_request(&PluginIpcRequest::CreateChatStream {
                request_id: request_id.clone(),
                provider_kind,
                provider_config,
                model,
                max_tokens,
                messages,
                tools,
            })
            .await
        {
            // Unregister the channel so a failed send does not leak it.
            self.router.streams.lock().remove(&request_id);
            return Err(e);
        }

        Ok(rx)
    }

    /// Unregisters a chat stream's channel with the router.
    ///
    /// Called when a stream completes or is dropped, releasing the per-request
    /// routing entry.
    pub fn close_chat_stream(&self, request_id: &str) {
        self.router.streams.lock().remove(request_id);
    }

    /// Sends a `ChatCompletion` request and awaits the result.
    ///
    /// Returns the assistant text plus any token usage the plugin reported;
    /// `usage` is `None` when the plugin does not report it (including
    /// older plugins that omit the field on the wire).
    pub async fn chat_completion(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<serde_json::Value>,
        json_schema: Option<serde_json::Value>,
    ) -> Result<(String, Option<ene_plugin_proto::TokenUsage>), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::ChatCompletion {
                request_id,
                provider_kind,
                provider_config,
                model,
                max_tokens,
                messages,
                json_schema,
            })
            .await?;
        match resp {
            PluginIpcResponse::ChatCompletionResult { content, usage, .. } => Ok((content, usage)),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ChatCompletion: {other:?}"
            ))),
        }
    }

    /// Sends an `EmbedBatch` request and awaits the result.
    ///
    /// Returns the per-item embeddings in input order; the plugin validates
    /// nothing about dimensions (the caller does), so a provider that
    /// ignores `dimensions` is fine.
    pub async fn embed_batch(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        model: String,
        dimensions: Option<u32>,
        items: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::EmbedBatch {
                request_id,
                provider_kind,
                provider_config,
                model,
                dimensions,
                items,
            })
            .await?;
        match resp {
            PluginIpcResponse::EmbedBatchResult { embeddings, .. } => Ok(embeddings),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to EmbedBatch: {other:?}"
            ))),
        }
    }

    /// Sends a `SynthesizeSpeech` request and awaits the result.
    ///
    /// Returns the base64-encoded audio bytes and the audio format echoed by
    /// the plugin. The caller decodes the payload; this layer only
    /// correlates the response.
    pub async fn synthesize_speech(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        text: String,
        voice: String,
        format: String,
    ) -> Result<(String, String), PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::SynthesizeSpeech {
                request_id,
                provider_kind,
                provider_config,
                text,
                voice,
                format,
            })
            .await?;
        match resp {
            PluginIpcResponse::SpeechResult {
                audio_base64,
                format,
                ..
            } => Ok((audio_base64, format)),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to SynthesizeSpeech: {other:?}"
            ))),
        }
    }

    /// Sends a `TranscribeAudio` request and awaits the transcribed text.
    ///
    /// The caller encodes the audio into the wire payload; this layer only
    /// correlates the response. The wire contract carries no language or
    /// duration, so the host adapter derives what it can from the PCM it
    /// encoded.
    pub async fn transcribe_audio(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        audio_base64: String,
        format: String,
    ) -> Result<String, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::TranscribeAudio {
                request_id,
                provider_kind,
                provider_config,
                audio_base64,
                format,
            })
            .await?;
        match resp {
            PluginIpcResponse::TranscriptionResult { text, .. } => Ok(text),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to TranscribeAudio: {other:?}"
            ))),
        }
    }

    /// Sends a `ProcessVadChunk` request and awaits the VAD event.
    ///
    /// Only callable after [`supports_vad`](Self::supports_vad) confirmed
    /// the peer knows the message; the manager gates factory registration on
    /// that check.
    pub async fn process_vad_chunk(
        &self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        session_id: String,
        pcm: Vec<f32>,
        reset: bool,
    ) -> Result<ene_plugin_proto::VadEvent, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::ProcessVadChunk {
                request_id,
                provider_kind,
                provider_config,
                session_id,
                pcm,
                reset,
            })
            .await?;
        match resp {
            PluginIpcResponse::VadChunkResult { event, .. } => Ok(event),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ProcessVadChunk: {other:?}"
            ))),
        }
    }

    /// Sends a graceful `Shutdown` request (best-effort; ignores errors).
    pub async fn shutdown(&self) {
        drop(self.send_request(&PluginIpcRequest::Shutdown).await);
    }

    /// Reconnects to the plugin binary unconditionally, re-performing the
    /// handshake.
    ///
    /// Uses the stored socket path, sandbox, and plugin config captured at
    /// the original [`connect`](Self::connect) call. Useful after a supervised
    /// process restart, where the caller knows the old connection is stale
    /// regardless of the generation (see the per-plugin supervisor in
    /// `manager`). Request-path reconnects go through
    /// [`reconnect_from`](Self::reconnect_from) instead, which coalesces
    /// concurrent reconnects by generation.
    pub async fn reconnect(&self) -> Result<(), PluginHostError> {
        self.reconnect_from(self.generation.load(Ordering::Acquire))
            .await
    }

    /// Reconnects to the plugin binary, re-performing the handshake, unless a
    /// sibling request already reconnected since `seen_generation`.
    ///
    /// `seen_generation` is the generation the caller observed before its
    /// request failed. If the current generation differs, another request
    /// already re-established the connection and this call returns `Ok(())`
    /// without touching it — the caller simply retries on the fresh connection.
    ///
    /// Aborts the old reader task and spawns a new one on the fresh stream.
    /// The aborted reader task is dropped at its suspension point inside
    /// `read_plugin_response` and never reaches the trailing `fail_all()` in
    /// [`reader_loop`], so this method fails all pending waiters *itself* —
    /// while still holding the writer lock — so in-flight callers observe a
    /// transport failure promptly instead of blocking until their own timeout.
    ///
    /// Takes `&self` (interior mutability): the writer and reader-task handles
    /// are swapped behind their own locks. The writer lock is held across the
    /// whole swap, so the generation re-check, the `fail_all()`, and the
    /// stream replacement are atomic with respect to a concurrent
    /// [`request_once`](Self::request_once): a request can neither write to a
    /// half-replaced stream nor register a waiter that survives the swap.
    /// On a failed reconnect attempt the writer stays `None` (the
    /// stale stream is already gone) and every request fails with
    /// "not connected" until a later reconnect succeeds.
    pub async fn reconnect_from(&self, seen_generation: u64) -> Result<(), PluginHostError> {
        let name = self.socket_path.file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        // Hold the writer lock across the swap so no concurrent request writes
        // to the stale stream, registers a waiter, or writes to a half-replaced
        // connection.
        let mut writer_guard = self.writer.lock().await;

        // Re-check the generation under the lock: a sibling that failed on the
        // same stale connection may have reconnected while we waited for the
        // lock. If so, its reconnect already failed the stale waiters and
        // installed a fresh stream — tearing it down again would fail the
        // siblings that already retried onto it.
        if self.generation.load(Ordering::Acquire) != seen_generation {
            return Ok(());
        }

        // Abort the old reader task so it stops reading from the stale stream.
        if let Some(task) = self.reader_task.lock().await.take() {
            task.abort();
        }
        *writer_guard = None;

        // The aborted task never reaches its trailing `fail_all()`, so fail
        // every pending waiter and close every stream channel here. Doing this
        // under the writer lock keeps it atomic with the stream swap: no
        // request can register a waiter between the fail and the replacement.
        self.router.fail_all();

        let mut stream = Self::connect_with_retry(&self.socket_path, &name).await?;

        // Clone under the parking_lot guards *before* awaiting so the
        // non-Send read guards are not held across `.await`.
        let plugin_config = self.plugin_config.read().clone();
        let plugin_profiles = self.plugin_profiles.read().clone();
        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange::host_supported(),
                sandbox: self.sandbox.clone(),
                plugin_config,
                plugin_profiles,
            },
            WireFormat::Json,
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake on reconnect: {e}"),
        })?;

        let resp = tokio::time::timeout(
            self.handshake_timeout,
            read_plugin_response(&mut stream, WireFormat::Json),
        )
        .await
        .map_err(|_| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!(
                "no HandshakeAck within {} ms on reconnect (plugin accepted the \
                     socket but never responded; defer heavy initialization until \
                     after the handshake)",
                self.handshake_timeout.as_millis()
            ),
        })?
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to read HandshakeAck on reconnect: {e}"),
        })?;

        match resp {
            Some(PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            }) => {
                // Accept any negotiated version inside our advertised range
                // (the plugin picks the highest mutually-supported version).
                let host_range = VersionRange::host_supported();
                if !host_range.contains(version) {
                    return Err(PluginHostError::ProtocolMismatch {
                        name,
                        host_min: host_range.min,
                        host_max: host_range.max,
                        got: version,
                    });
                }
                *self.capabilities.write() = capabilities;
                self.negotiated_version.store(version, Ordering::Release);
            }
            Some(PluginIpcResponse::Error { message, .. }) => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: format!(
                        "{message} (host supports protocol {}..={})",
                        VersionRange::host_supported().min,
                        VersionRange::host_supported().max
                    ),
                });
            }
            _ => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: "unexpected response to Handshake on reconnect".to_string(),
                });
            }
        }

        let (reader, writer) = tokio::io::split(stream);
        let reader_task = tokio::spawn(reader_loop(
            reader,
            Arc::clone(&self.router),
            WireFormat::for_version(self.negotiated_version()),
        ));
        *writer_guard = Some(writer);
        *self.reader_task.lock().await = Some(reader_task);

        // Advance the generation so concurrent requests that failed on the old
        // connection coalesce onto this one instead of each reconnecting again.
        self.generation.fetch_add(1, Ordering::AcqRel);

        Ok(())
    }

    // ── Internal helpers ──

    /// Validates that a response is the expected [`PluginIpcResponse::Ack`],
    /// mapping anything else to an execution error.
    fn expect_ack(resp: PluginIpcResponse, what: &str) -> Result<(), PluginHostError> {
        match resp {
            PluginIpcResponse::Ack { .. } => Ok(()),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to {what}: {other:?}"
            ))),
        }
    }

    /// Injects a `request_id` into a [`PluginIpcRequest`] in-place.
    ///
    /// This match is deliberately exhaustive (no `_` catch-all) so that adding
    /// a new request variant that carries a `request_id` produces a compile
    /// error here, forcing the author to keep this in sync with
    /// [`request_request_id`](Self::request_request_id). The two functions must
    /// always agree on which variants carry a `request_id`; the
    /// `inject_and_request_id_arm_sets_agree` test enforces this at runtime.
    fn inject_request_id(req: &mut PluginIpcRequest, request_id: &str) {
        match req {
            PluginIpcRequest::GetConfigSchema { request_id: rid }
            | PluginIpcRequest::ListTools { request_id: rid }
            | PluginIpcRequest::CallTool {
                request_id: rid, ..
            }
            | PluginIpcRequest::SetCallContext {
                request_id: rid, ..
            }
            | PluginIpcRequest::ApprovePermission {
                request_id: rid, ..
            }
            | PluginIpcRequest::AllowPattern {
                request_id: rid, ..
            }
            | PluginIpcRequest::RevokePattern {
                request_id: rid, ..
            }
            | PluginIpcRequest::PollDeferred {
                request_id: rid, ..
            }
            | PluginIpcRequest::CancelDeferred {
                request_id: rid, ..
            }
            | PluginIpcRequest::CancelStream {
                request_id: rid, ..
            }
            | PluginIpcRequest::CreateChatStream {
                request_id: rid, ..
            }
            | PluginIpcRequest::ChatCompletion {
                request_id: rid, ..
            }
            | PluginIpcRequest::EmbedBatch {
                request_id: rid, ..
            }
            | PluginIpcRequest::SynthesizeSpeech {
                request_id: rid, ..
            }
            | PluginIpcRequest::TranscribeAudio {
                request_id: rid, ..
            }
            | PluginIpcRequest::ProcessVadChunk {
                request_id: rid, ..
            }
            | PluginIpcRequest::SetConfig {
                request_id: rid, ..
            }
            | PluginIpcRequest::ListConfigOptions {
                request_id: rid, ..
            }
            | PluginIpcRequest::ValidateConfig {
                request_id: rid, ..
            }
            | PluginIpcRequest::MigrateConfig {
                request_id: rid, ..
            }
            | PluginIpcRequest::Ping { request_id: rid } => {
                *rid = request_id.to_string();
            }
            // Variants without a `request_id` field; nothing to inject.
            PluginIpcRequest::Handshake { .. } | PluginIpcRequest::Shutdown => {}
        }
    }

    /// Verifies that a response's `request_id` matches the expected value.
    ///
    /// Push messages ([`DeferredCompleted`](PluginIpcResponse::DeferredCompleted))
    /// and [`HandshakeAck`](PluginIpcResponse::HandshakeAck) carry no
    /// `request_id`; they are routed separately by the single reader task and
    /// never reach request/response correlation, so they are accepted here
    /// without a match.
    fn verify_request_id(resp: &PluginIpcResponse, expected: &str) -> Result<(), PluginHostError> {
        let Some(actual) = response_request_id(resp) else {
            return Ok(());
        };
        if !actual.is_empty() && actual != expected {
            return Err(PluginHostError::execution(format!(
                "request_id mismatch: expected {expected}, got {actual}"
            )));
        }
        Ok(())
    }

    async fn connect_with_retry(
        socket_path: &Path,
        name: &str,
    ) -> Result<IpcStream, PluginHostError> {
        let mut attempts = 0_u32;
        loop {
            match IpcStream::connect(socket_path).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempts = attempts.saturating_add(1);
                    if attempts >= CONNECT_MAX_RETRIES {
                        return Err(PluginHostError::ConnectFailed {
                            name: name.to_string(),
                            reason: format!("failed after {attempts} attempts: {e}"),
                        });
                    }
                    tokio::time::sleep(CONNECT_DELAY).await;
                }
            }
        }
    }

    /// Sends a request and reads a single response with the default timeout.
    async fn do_request(
        &self,
        req: PluginIpcRequest,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        self.do_request_with_timeout(req, self.timeout).await
    }

    /// Returns the `request_id` from a request variant, or `None` if the
    /// variant does not carry a `request_id` field.
    fn request_request_id(req: &PluginIpcRequest) -> Option<&str> {
        match req {
            PluginIpcRequest::GetConfigSchema { request_id }
            | PluginIpcRequest::ListTools { request_id }
            | PluginIpcRequest::CallTool { request_id, .. }
            | PluginIpcRequest::SetCallContext { request_id, .. }
            | PluginIpcRequest::ApprovePermission { request_id, .. }
            | PluginIpcRequest::AllowPattern { request_id, .. }
            | PluginIpcRequest::RevokePattern { request_id, .. }
            | PluginIpcRequest::PollDeferred { request_id, .. }
            | PluginIpcRequest::CancelDeferred { request_id, .. }
            | PluginIpcRequest::CancelStream { request_id, .. }
            | PluginIpcRequest::CreateChatStream { request_id, .. }
            | PluginIpcRequest::ChatCompletion { request_id, .. }
            | PluginIpcRequest::EmbedBatch { request_id, .. }
            | PluginIpcRequest::SynthesizeSpeech { request_id, .. }
            | PluginIpcRequest::TranscribeAudio { request_id, .. }
            | PluginIpcRequest::ProcessVadChunk { request_id, .. }
            | PluginIpcRequest::SetConfig { request_id, .. }
            | PluginIpcRequest::ListConfigOptions { request_id, .. }
            | PluginIpcRequest::ValidateConfig { request_id, .. }
            | PluginIpcRequest::MigrateConfig { request_id, .. }
            | PluginIpcRequest::Ping { request_id } => Some(request_id.as_str()),
            PluginIpcRequest::Handshake { .. } | PluginIpcRequest::Shutdown => None,
        }
    }

    /// Sends a request and reads a single response with an explicit timeout.
    ///
    /// Generates a UUID for non-streaming requests that don't already carry a
    /// `request_id` and verifies the response's `request_id` matches, enabling
    /// concurrent in-flight requests.
    ///
    /// On a transport failure (broken pipe, connection reset, EOF) the stale
    /// stream is dropped, the connection is re-established via
    /// [`reconnect`](Self::reconnect), and the request is retried **once**.
    /// This is safe only for the request/response pattern: a transport error
    /// means the request never reached (or was never answered by) the plugin,
    /// so replaying it cannot double-execute a call. Timeouts are deliberately
    /// **not** retried — a timed-out plugin may still be processing a
    /// non-idempotent call. Streaming reads bypass this path entirely and
    /// never trigger reconnection mid-stream.
    async fn do_request_with_timeout(
        &self,
        mut req: PluginIpcRequest,
        timeout: Duration,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        let request_id = if let Some(rid) = Self::request_request_id(&req) {
            if rid.is_empty() {
                let rid = next_request_id();
                Self::inject_request_id(&mut req, &rid);
                rid
            } else {
                rid.to_string()
            }
        } else {
            let rid = next_request_id();
            Self::inject_request_id(&mut req, &rid);
            rid
        };

        // Bound concurrent in-flight requests against this connection. The
        // permit is acquired once and held across the whole logical
        // request — including the single reconnect-and-retry — so a retry never
        // re-acquires and a saturated connection queues rather than fanning
        // out unboundedly. The acquire itself is bounded by the request
        // timeout: a saturated connection must not stall the caller (e.g. a
        // 5 s `Ping` liveness probe) indefinitely waiting for a slot. Each
        // plugin is supervised by its own task, so a stalled probe delays
        // only that plugin's supervisor rather than every plugin's. Released
        // on every exit path via drop.
        let permit = tokio::time::timeout(timeout, self.inflight.clone().acquire_owned())
            .await
            .map_err(|_| {
                PluginHostError::execution(format!(
                    "timed out after {} ms waiting for a connection slot",
                    timeout.as_millis()
                ))
            })?
            .map_err(|_| PluginHostError::transport("connection concurrency limiter closed"))?;

        // Snapshot the connection generation before the first attempt. If the
        // transport fails and a sibling request has already reconnected in the
        // meantime (advancing the generation), this request simply retries on
        // the fresh connection instead of reconnecting a second time and
        // tearing down the sibling's work.
        let generation_before = self.generation.load(Ordering::Acquire);

        match self.request_once(&req, &request_id, timeout, &permit).await {
            Ok(resp) => {
                Self::verify_request_id(&resp, &request_id)?;
                Ok(resp)
            }
            Err(e @ PluginHostError::TransportFailed { .. }) => {
                if self.generation.load(Ordering::Acquire) == generation_before {
                    tracing::warn!(
                        component = "IpcPluginConnection",
                        error = %e,
                        "Transport failure; reconnecting and retrying request once"
                    );
                } else {
                    tracing::debug!(
                        component = "IpcPluginConnection",
                        error = %e,
                        "Transport failure; a concurrent request already reconnected, retrying"
                    );
                }
                // `reconnect_from` re-checks the generation under the writer
                // lock, so even if two siblings race past the logging branch
                // above, only the first one actually reconnects.
                self.reconnect_from(generation_before).await?;
                let resp = self
                    .request_once(&req, &request_id, timeout, &permit)
                    .await?;
                Self::verify_request_id(&resp, &request_id)?;
                Ok(resp)
            }
            Err(e) => Err(e),
        }
    }

    /// Performs a single request/response round-trip using the router's
    /// oneshot waiters.
    ///
    /// Registers the oneshot waiter and writes the request atomically under
    /// the short-lived writer lock (see
    /// [`send_request_registering`](Self::send_request_registering)),
    /// **releases that lock**, and only then awaits the waiter with a timeout.
    /// The single reader task routes the response to the waiter by
    /// `request_id`, so the connection is free to carry other in-flight
    /// requests during the wait.
    ///
    /// `_permit` is this request's slot in the connection-level concurrency
    /// bound, acquired by [`do_request_with_timeout`](Self::do_request_with_timeout);
    /// it is held (not re-acquired) here so the bound spans the retry too.
    ///
    /// Classifies transport failures as [`PluginHostError::TransportFailed`]
    /// so the caller can decide whether a reconnect-and-retry is warranted.
    async fn request_once(
        &self,
        req: &PluginIpcRequest,
        request_id: &str,
        timeout: Duration,
        _permit: &tokio::sync::OwnedSemaphorePermit,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        let rx = self.send_request_registering(req, request_id).await?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(PluginHostError::transport(
                "reader task exited (connection closed)",
            )),
            Err(_elapsed) => {
                self.router.waiters.lock().remove(request_id);
                Err(PluginHostError::execution(format!(
                    "timed out after {} ms",
                    timeout.as_millis()
                )))
            }
        }
    }

    /// Writes a request to the stream (no read, no waiter).
    ///
    /// Used only for fire-and-forget sends whose responses are not correlated
    /// through the router's waiters — [`shutdown`](Self::shutdown) and
    /// [`send_create_chat_stream`](Self::send_create_chat_stream) (which
    /// registers a *stream* channel instead). Request/response round-trips go
    /// through [`send_request_registering`](Self::send_request_registering).
    ///
    /// The writer lock is held only for the duration of the frame write, so
    /// concurrent requests serialize their writes (frames never interleave)
    /// without blocking each other's response waits.
    ///
    /// A missing stream or a failed write is reported as
    /// [`PluginHostError::TransportFailed`], which the request path treats as
    /// reconnectable.
    async fn send_request(&self, req: &PluginIpcRequest) -> Result<(), PluginHostError> {
        let mut writer = self.writer.lock().await;
        let writer = writer
            .as_mut()
            .ok_or_else(|| PluginHostError::transport("not connected to plugin"))?;
        write_plugin_request(writer, req, self.wire_format())
            .await
            .map_err(|e| PluginHostError::transport(format!("write failed: {e}")))
    }

    /// Registers the response waiter and writes the request atomically under
    /// the writer lock, returning the receiver to await.
    ///
    /// Registration and the frame write happen inside one writer-lock critical
    /// section so they are atomic with respect to
    /// [`reconnect_from`](Self::reconnect_from), which holds the same lock
    /// while it fails all waiters and swaps the stream. This closes the
    /// window where a waiter registered *before* the lock could survive a
    /// reconnect's `fail_all()`, get written to the fresh stream, and then be
    /// replayed by the retry path — double-executing a non-idempotent
    /// `CallTool` and breaking the "a transport error means the request never
    /// reached the plugin" retry invariant.
    ///
    /// On a failed write the just-registered waiter is removed again so a
    /// stale entry cannot leak into a later response.
    async fn send_request_registering(
        &self,
        req: &PluginIpcRequest,
        request_id: &str,
    ) -> Result<oneshot::Receiver<PluginIpcResponse>, PluginHostError> {
        let mut writer = self.writer.lock().await;
        let writer = writer
            .as_mut()
            .ok_or_else(|| PluginHostError::transport("not connected to plugin"))?;

        let (tx, rx) = oneshot::channel();
        self.router
            .waiters
            .lock()
            .insert(request_id.to_string(), tx);

        match write_plugin_request(writer, req, self.wire_format()).await {
            Ok(()) => Ok(rx),
            Err(e) => {
                self.router.waiters.lock().remove(request_id);
                Err(PluginHostError::transport(format!("write failed: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns one sample of every [`PluginIpcRequest`] variant.
    ///
    /// Because this constructs each variant explicitly, adding a new variant
    /// produces a compile error here, forcing the author to extend the sample
    /// (and, transitively, the `inject_request_id` / `request_request_id`
    /// arm-set agreement test below).
    fn all_request_variants() -> Vec<PluginIpcRequest> {
        vec![
            PluginIpcRequest::Handshake {
                version: VersionRange { min: 4, max: 4 },
                sandbox: SandboxConfigData::default(),
                plugin_config: None,
                plugin_profiles: None,
            },
            PluginIpcRequest::Shutdown,
            PluginIpcRequest::Ping {
                request_id: String::new(),
            },
            PluginIpcRequest::GetConfigSchema {
                request_id: String::new(),
            },
            PluginIpcRequest::ListTools {
                request_id: String::new(),
            },
            PluginIpcRequest::CallTool {
                request_id: String::new(),
                name: "t".into(),
                arguments: "{}".into(),
                deferred: false,
                context: None,
            },
            PluginIpcRequest::SetCallContext {
                request_id: String::new(),
                conversation_id: "c".into(),
                turn_id: "t".into(),
            },
            PluginIpcRequest::PollDeferred {
                request_id: String::new(),
                task_id: "task".into(),
            },
            PluginIpcRequest::CancelStream {
                request_id: String::new(),
                stream_request_id: "s".into(),
            },
            PluginIpcRequest::CancelDeferred {
                request_id: String::new(),
                task_id: "task".into(),
            },
            PluginIpcRequest::ApprovePermission {
                request_id: String::new(),
                permission_request_id: "p".into(),
            },
            PluginIpcRequest::AllowPattern {
                request_id: String::new(),
                action: "a".into(),
                target_pattern: "t".into(),
            },
            PluginIpcRequest::RevokePattern {
                request_id: String::new(),
                action: "a".into(),
                target_pattern: "t".into(),
            },
            PluginIpcRequest::CreateChatStream {
                request_id: String::new(),
                provider_kind: "k".into(),
                provider_config: serde_json::Value::Null,
                model: "m".into(),
                max_tokens: None,
                messages: Vec::new(),
                tools: Vec::new(),
            },
            PluginIpcRequest::ChatCompletion {
                request_id: String::new(),
                provider_kind: "k".into(),
                provider_config: serde_json::Value::Null,
                model: "m".into(),
                max_tokens: None,
                messages: Vec::new(),
                json_schema: None,
            },
            PluginIpcRequest::SynthesizeSpeech {
                request_id: String::new(),
                provider_kind: "k".into(),
                provider_config: serde_json::Value::Null,
                text: "hi".into(),
                voice: "v".into(),
                format: "wav".into(),
            },
            PluginIpcRequest::TranscribeAudio {
                request_id: String::new(),
                provider_kind: "k".into(),
                provider_config: serde_json::Value::Null,
                audio_base64: "AAAA".into(),
                format: "wav".into(),
            },
            PluginIpcRequest::EmbedBatch {
                request_id: String::new(),
                provider_kind: "k".into(),
                provider_config: serde_json::Value::Null,
                model: "m".into(),
                dimensions: None,
                items: Vec::new(),
            },
            PluginIpcRequest::SetConfig {
                request_id: String::new(),
                config: serde_json::json!({}),
                profiles: None,
            },
            PluginIpcRequest::ListConfigOptions {
                request_id: String::new(),
                path: "voice".into(),
            },
            PluginIpcRequest::ValidateConfig {
                request_id: String::new(),
                value: serde_json::json!({}),
            },
            PluginIpcRequest::MigrateConfig {
                request_id: String::new(),
                from_version: 0,
                value: serde_json::json!({}),
            },
        ]
    }

    /// `inject_request_id` must write a `request_id` for exactly the variants
    /// that `request_request_id` reports as carrying one. If a future variant
    /// is added to one function but not the other, request/response correlation
    /// silently breaks; this test fails in that case.
    #[test]
    fn inject_and_request_id_arm_sets_agree() {
        const SENTINEL: &str = "injected-id";
        for mut req in all_request_variants() {
            let carries = IpcPluginConnection::request_request_id(&req).is_some();

            IpcPluginConnection::inject_request_id(&mut req, SENTINEL);
            let injected =
                IpcPluginConnection::request_request_id(&req).is_some_and(|rid| rid == SENTINEL);

            assert_eq!(
                carries, injected,
                "inject_request_id and request_request_id disagree for variant: {req:?}"
            );
        }
    }
}
