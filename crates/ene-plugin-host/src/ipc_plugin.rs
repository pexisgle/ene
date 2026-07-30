//! IPC client to a single plugin binary.
//!
//! [`IpcPluginConnection`] manages the lifecycle of one connection to a
//! plugin process: handshake, request/response, and reconnection on failure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, IpcStream, PluginCapabilities, PluginIpcRequest,
    PluginIpcResponse, SandboxConfigData, ToolError, ToolResult, VersionRange,
    read_plugin_response, write_plugin_request,
};
use tokio::io::{ReadHalf, WriteHalf};
use tokio::sync::{mpsc, oneshot};
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

/// Shared routing state for the single reader task (H-12).
///
/// Every incoming [`PluginIpcResponse`] is dispatched here by the reader task
/// *before* any request/response correlation, so push messages
/// ([`DeferredCompleted`](PluginIpcResponse::DeferredCompleted)) and stream
/// messages can never be stolen by an unrelated in-flight request (Cr-5).
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
    /// arrived while the connection was idle is still delivered (Cr-5).
    deferred: parking_lot::Mutex<HashMap<String, Result<ToolResult, ToolError>>>,
}

impl Router {
    fn new() -> Self {
        Self {
            waiters: parking_lot::Mutex::new(HashMap::new()),
            streams: parking_lot::Mutex::new(HashMap::new()),
            deferred: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Routes a single response to its destination.
    ///
    /// Push and stream messages are matched by variant *first*, so they can
    /// never fall through to request/response correlation (Cr-5).
    fn dispatch(&self, resp: PluginIpcResponse) {
        match &resp {
            // Push: cache the deferred completion for later retrieval.
            PluginIpcResponse::DeferredCompleted { task_id, result } => {
                self.deferred.lock().insert(task_id.clone(), result.clone());
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
/// Push messages ([`DeferredCompleted`](PluginIpcResponse::DeferredCompleted))
/// and [`HandshakeAck`](PluginIpcResponse::HandshakeAck) carry no
/// `request_id` and return `None`.
fn response_request_id(resp: &PluginIpcResponse) -> Option<&str> {
    match resp {
        PluginIpcResponse::HandshakeAck { .. } | PluginIpcResponse::DeferredCompleted { .. } => {
            None
        }
        PluginIpcResponse::Ack { request_id }
        | PluginIpcResponse::Pong { request_id }
        | PluginIpcResponse::ConfigSchema { request_id, .. }
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
        | PluginIpcResponse::TranscriptionResult { request_id, .. } => Some(request_id.as_str()),
    }
}

/// The single reader task for a connection (H-12).
///
/// Reads responses from the socket in a loop and dispatches each one through
/// the [`Router`]. Exits on EOF or read error, then fails all pending waiters
/// so in-flight callers observe the transport failure promptly.
async fn reader_loop(mut reader: ReadHalf<IpcStream>, router: Arc<Router>) {
    loop {
        match read_plugin_response(&mut reader).await {
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
/// reconnection on transport failure. All methods are async; the caller
/// is expected to serialize access (e.g. via `tokio::sync::Mutex`).
///
/// ## Single reader task (H-12)
///
/// A dedicated reader task reads all responses from the socket and dispatches
/// them through a [`Router`] that routes by `request_id`/variant *before*
/// request/response correlation. This eliminates the "between-reads lock
/// release" pattern and ensures push messages (`DeferredCompleted`) and stream
/// messages are never stolen by an unrelated in-flight request.
pub struct IpcPluginConnection {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    plugin_config: Option<serde_json::Value>,
    /// Write half of the IPC stream. The read half is owned by the reader task.
    writer: Option<WriteHalf<IpcStream>>,
    capabilities: PluginCapabilities,
    /// The protocol version negotiated with the plugin during the handshake
    /// (see [`VersionRange::negotiate`]). Used to gate host behavior that
    /// depends on a message variant introduced after v3 — see
    /// [`supports_cancel_stream`](Self::supports_cancel_stream) for the
    /// documented pattern to follow when adding another gate.
    negotiated_version: u32,
    timeout: Duration,
    /// Timeout applied to the handshake response read. Captured at
    /// [`connect`](Self::connect) time so [`reconnect`](Self::reconnect)
    /// reuses the same bound without re-reading configuration.
    handshake_timeout: Duration,
    /// Shared routing state for the reader task.
    router: Arc<Router>,
    /// Handle to the reader task. Aborted on reconnect/shutdown.
    reader_task: Option<JoinHandle<()>>,
}

impl IpcPluginConnection {
    /// Connects to a plugin binary at `socket_path`, performs the protocol
    /// handshake (advertising the host's supported version range), and stores
    /// the advertised capabilities.
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
        handshake_timeout: Duration,
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
            },
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake: {e}"),
        })?;

        let resp = tokio::time::timeout(handshake_timeout, read_plugin_response(&mut stream))
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
        let reader_task = tokio::spawn(reader_loop(reader, Arc::clone(&router)));

        Ok(Self {
            socket_path: socket_path.to_path_buf(),
            sandbox,
            plugin_config,
            writer: Some(writer),
            capabilities,
            negotiated_version,
            timeout: DEFAULT_TIMEOUT,
            handshake_timeout,
            router,
            reader_task: Some(reader_task),
        })
    }

    /// Returns the capabilities advertised by the plugin during the handshake.
    pub const fn capabilities(&self) -> &PluginCapabilities {
        &self.capabilities
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
    pub const fn negotiated_version(&self) -> u32 {
        self.negotiated_version
    }

    /// Returns whether the negotiated protocol version supports explicit
    /// stream cancellation via [`PluginIpcRequest::CancelStream`].
    ///
    /// `CancelStream` was introduced in protocol v4 (see the
    /// `PLUGIN_IPC_PROTOCOL_VERSION` docs in `ene-plugin-proto`). A plugin
    /// that negotiated v3 does not know this message variant, so
    /// [`cancel_stream`](Self::cancel_stream) skips sending it and relies on
    /// the caller's existing timeout-based fallback to end the stream
    /// instead.
    ///
    /// This is the pattern to follow for any future version-gated feature:
    /// add a `const fn supports_x(&self) -> bool` here that compares
    /// `self.negotiated_version` against the version the feature was
    /// introduced in, and branch on it wherever the feature is used.
    pub const fn supports_cancel_stream(&self) -> bool {
        self.negotiated_version >= CANCEL_STREAM_MIN_VERSION
    }

    /// Sends a `ListTools` request and returns the actual tool specs.
    pub async fn list_tools(&mut self) -> Result<Vec<ene_plugin_proto::ToolSpec>, PluginHostError> {
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

    /// Sends a `Ping` and waits for `Pong` within [`PING_TIMEOUT`].
    pub async fn ping(&mut self) -> Result<(), PluginHostError> {
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
        &mut self,
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
    pub async fn set_call_context(&mut self, ctx: &CallContext) -> Result<(), PluginHostError> {
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
        &mut self,
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
        &mut self,
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
        &mut self,
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
        &mut self,
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
    /// was idle is delivered promptly (Cr-5).
    pub async fn poll_deferred(
        &mut self,
        task_id: &str,
    ) -> Result<DeferredStatus, PluginHostError> {
        // Check the push cache first (Cr-5 idle delivery).
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
    pub async fn cancel_deferred(&mut self, task_id: &str) -> Result<(), PluginHostError> {
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
    pub async fn cancel_stream(&mut self, request_id: &str) -> Result<(), PluginHostError> {
        if !self.supports_cancel_stream() {
            tracing::debug!(
                component = "IpcPluginConnection",
                negotiated_version = self.negotiated_version,
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

    /// Sends a `CreateChatStream` request and returns a receiver for the
    /// stream's responses.
    ///
    /// A per-request channel is registered with the [`Router`] *before* the
    /// request is written, so the single reader task routes every
    /// `StreamChunk` / `StreamEnd` / `StreamError` for this `request_id` into
    /// the returned receiver (H-12). The receiver yields responses until a
    /// terminal `StreamEnd`/`StreamError` is observed or the channel closes
    /// (connection failure or stream cancellation).
    pub async fn send_create_chat_stream(
        &mut self,
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
    pub fn close_chat_stream(&mut self, request_id: &str) {
        self.router.streams.lock().remove(request_id);
    }

    /// Sends a `ChatCompletion` request and awaits the result.
    pub async fn chat_completion(
        &mut self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<serde_json::Value>,
        json_schema: Option<serde_json::Value>,
    ) -> Result<String, PluginHostError> {
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
            PluginIpcResponse::ChatCompletionResult { content, .. } => Ok(content),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to ChatCompletion: {other:?}"
            ))),
        }
    }

    /// Sends a graceful `Shutdown` request (best-effort; ignores errors).
    pub async fn shutdown(&mut self) {
        drop(self.send_request(&PluginIpcRequest::Shutdown).await);
    }

    /// Reconnects to the plugin binary, re-performing the handshake.
    ///
    /// Uses the stored socket path, sandbox, and plugin config captured at
    /// the original [`connect`](Self::connect) call. Useful after a transport
    /// failure or a supervised process restart.
    ///
    /// Aborts the old reader task and spawns a new one on the fresh stream.
    /// All pending waiters are failed by the old reader task's exit.
    pub async fn reconnect(&mut self) -> Result<(), PluginHostError> {
        let name = self.socket_path.file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        // Abort the old reader task so it stops reading from the stale stream.
        if let Some(task) = self.reader_task.take() {
            task.abort();
        }
        self.writer = None;

        let mut stream = Self::connect_with_retry(&self.socket_path, &name).await?;

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange::host_supported(),
                sandbox: self.sandbox.clone(),
                plugin_config: self.plugin_config.clone(),
            },
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake on reconnect: {e}"),
        })?;

        let resp = tokio::time::timeout(self.handshake_timeout, read_plugin_response(&mut stream))
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
                self.capabilities = capabilities;
                self.negotiated_version = version;
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

        // Spawn a new reader task on the fresh stream.
        let (reader, writer) = tokio::io::split(stream);
        let reader_task = tokio::spawn(reader_loop(reader, Arc::clone(&self.router)));
        self.writer = Some(writer);
        self.reader_task = Some(reader_task);

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
    /// without a match (Cr-5).
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
        &mut self,
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
        &mut self,
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
        match self.request_once(&req, &request_id, timeout).await {
            Ok(resp) => {
                Self::verify_request_id(&resp, &request_id)?;
                Ok(resp)
            }
            Err(e @ PluginHostError::TransportFailed { .. }) => {
                tracing::warn!(
                    component = "IpcPluginConnection",
                    error = %e,
                    "Transport failure; reconnecting and retrying request once"
                );
                self.reconnect().await?;
                let resp = self.request_once(&req, &request_id, timeout).await?;
                Self::verify_request_id(&resp, &request_id)?;
                Ok(resp)
            }
            Err(e) => Err(e),
        }
    }

    /// Performs a single request/response round-trip using the router's
    /// oneshot waiters.
    ///
    /// Registers a oneshot waiter with the router *before* sending the request,
    /// then sends the request and awaits the waiter with a timeout. The single
    /// reader task routes the response to the waiter by `request_id`.
    ///
    /// Classifies transport failures as [`PluginHostError::TransportFailed`]
    /// so the caller can decide whether a reconnect-and-retry is warranted.
    async fn request_once(
        &mut self,
        req: &PluginIpcRequest,
        request_id: &str,
        timeout: Duration,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        let (tx, rx) = oneshot::channel();
        self.router
            .waiters
            .lock()
            .insert(request_id.to_string(), tx);

        if let Err(e) = self.send_request(req).await {
            self.router.waiters.lock().remove(request_id);
            return Err(e);
        }

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

    /// Writes a request to the stream (no read).
    ///
    /// A missing stream or a failed write is reported as
    /// [`PluginHostError::TransportFailed`], which the request path treats as
    /// reconnectable.
    async fn send_request(&mut self, req: &PluginIpcRequest) -> Result<(), PluginHostError> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| PluginHostError::transport("not connected to plugin"))?;
        write_plugin_request(writer, req)
            .await
            .map_err(|e| PluginHostError::transport(format!("write failed: {e}")))
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
        ]
    }

    /// `inject_request_id` must write a `request_id` for exactly the variants
    /// that `request_request_id` reports as carrying one. If a future variant
    /// is added to one function but not the other, request/response correlation
    /// silently breaks (H-9 regression); this test fails in that case.
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
