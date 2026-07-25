//! IPC client to a single plugin binary.
//!
//! [`IpcPluginConnection`] manages the lifecycle of one connection to a
//! plugin process: handshake, request/response, and reconnection on failure.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION,
    PluginCapabilities, PluginIpcRequest, PluginIpcResponse, SandboxConfigData, VersionRange,
    read_plugin_response, write_plugin_request,
};

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

/// An IPC connection to a single plugin binary.
///
/// Handles the handshake, request/response round-trips, and transparent
/// reconnection on transport failure. All methods are async; the caller
/// is expected to serialize access (e.g. via `tokio::sync::Mutex`).
pub struct IpcPluginConnection {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    plugin_config: Option<serde_json::Value>,
    stream: Option<IpcStream>,
    capabilities: PluginCapabilities,
    timeout: Duration,
}

impl IpcPluginConnection {
    /// Connects to a plugin binary at `socket_path`, performs the v3
    /// handshake, and stores the advertised capabilities.
    ///
    /// Retries the connect up to [`CONNECT_MAX_RETRIES`] times with a
    /// fixed delay, giving the child process time to bind its listener.
    pub async fn connect(
        socket_path: &Path,
        sandbox: SandboxConfigData,
        plugin_config: Option<serde_json::Value>,
    ) -> Result<Self, PluginHostError> {
        let name = socket_path.file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        let mut stream = Self::connect_with_retry(socket_path, &name).await?;

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange {
                    min: PLUGIN_IPC_PROTOCOL_VERSION,
                    max: PLUGIN_IPC_PROTOCOL_VERSION,
                },
                sandbox: sandbox.clone(),
                plugin_config: plugin_config.clone(),
            },
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake: {e}"),
        })?;

        let resp = read_plugin_response(&mut stream).await.map_err(|e| {
            PluginHostError::HandshakeFailed {
                name: name.clone(),
                reason: format!("failed to read HandshakeAck: {e}"),
            }
        })?;

        let capabilities = match resp {
            Some(PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            }) => {
                if version != PLUGIN_IPC_PROTOCOL_VERSION {
                    return Err(PluginHostError::ProtocolMismatch {
                        name,
                        expected: PLUGIN_IPC_PROTOCOL_VERSION,
                        got: version,
                    });
                }
                capabilities
            }
            Some(PluginIpcResponse::Error { message, .. }) => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: message,
                });
            }
            _ => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: "unexpected response to Handshake".to_string(),
                });
            }
        };

        Ok(Self {
            socket_path: socket_path.to_path_buf(),
            sandbox,
            plugin_config,
            stream: Some(stream),
            capabilities,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Returns the capabilities advertised by the plugin during the handshake.
    pub const fn capabilities(&self) -> &PluginCapabilities {
        &self.capabilities
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
            .do_request_with_timeout(PluginIpcRequest::Ping, PING_TIMEOUT)
            .await?;
        match resp {
            PluginIpcResponse::Pong => Ok(()),
            PluginIpcResponse::Error { message, .. } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to Ping: {other:?}"
            ))),
        }
    }

    /// Calls a tool exposed by the plugin and returns the result string.
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
    ) -> Result<String, PluginHostError> {
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
    pub async fn poll_deferred(
        &mut self,
        task_id: &str,
    ) -> Result<DeferredStatus, PluginHostError> {
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

    /// Sends a `CreateChatStream` request (does not read responses).
    ///
    /// After calling this, use [`read_response`](Self::read_response) in a
    /// loop until a terminal `StreamEnd` or `StreamError` is observed.
    pub async fn send_create_chat_stream(
        &mut self,
        request_id: String,
        provider_kind: String,
        provider_config: serde_json::Value,
        model: String,
        max_tokens: Option<u32>,
        messages: Vec<serde_json::Value>,
        tools: Vec<serde_json::Value>,
    ) -> Result<(), PluginHostError> {
        self.send_request(&PluginIpcRequest::CreateChatStream {
            request_id,
            provider_kind,
            provider_config,
            model,
            max_tokens,
            messages,
            tools,
        })
        .await
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

    /// Reads the next response from the stream (used for streaming reads).
    ///
    /// Returns `Ok(None)` on EOF (connection closed).
    pub async fn read_response(&mut self) -> Result<Option<PluginIpcResponse>, PluginHostError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| PluginHostError::execution("not connected to plugin"))?;
        let fut = read_plugin_response(stream);
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => Err(PluginHostError::execution(format!("read failed: {e}"))),
            Err(_elapsed) => Err(PluginHostError::execution(format!(
                "read timed out after {} ms",
                self.timeout.as_millis()
            ))),
        }
    }

    /// Sends a graceful `Shutdown` request (best-effort; ignores errors).
    pub async fn shutdown(&mut self) {
        let _ = self.send_request(&PluginIpcRequest::Shutdown).await;
    }

    /// Reconnects to the plugin binary, re-performing the handshake.
    ///
    /// Uses the stored socket path, sandbox, and plugin config captured at
    /// the original [`connect`](Self::connect) call. Useful after a transport
    /// failure or a supervised process restart.
    pub async fn reconnect(&mut self) -> Result<(), PluginHostError> {
        let name = self.socket_path.file_name().map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().to_string(),
        );

        let mut stream = Self::connect_with_retry(&self.socket_path, &name).await?;

        write_plugin_request(
            &mut stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange {
                    min: PLUGIN_IPC_PROTOCOL_VERSION,
                    max: PLUGIN_IPC_PROTOCOL_VERSION,
                },
                sandbox: self.sandbox.clone(),
                plugin_config: self.plugin_config.clone(),
            },
        )
        .await
        .map_err(|e| PluginHostError::HandshakeFailed {
            name: name.clone(),
            reason: format!("failed to send Handshake on reconnect: {e}"),
        })?;

        let resp = read_plugin_response(&mut stream).await.map_err(|e| {
            PluginHostError::HandshakeFailed {
                name: name.clone(),
                reason: format!("failed to read HandshakeAck on reconnect: {e}"),
            }
        })?;

        match resp {
            Some(PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            }) => {
                if version != PLUGIN_IPC_PROTOCOL_VERSION {
                    return Err(PluginHostError::ProtocolMismatch {
                        name,
                        expected: PLUGIN_IPC_PROTOCOL_VERSION,
                        got: version,
                    });
                }
                self.capabilities = capabilities;
            }
            Some(PluginIpcResponse::Error { message, .. }) => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: message,
                });
            }
            _ => {
                return Err(PluginHostError::HandshakeFailed {
                    name,
                    reason: "unexpected response to Handshake on reconnect".to_string(),
                });
            }
        }

        self.stream = Some(stream);
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
            } => {
                *rid = request_id.to_string();
            }
            _ => {}
        }
    }

    /// Verifies that a response's `request_id` matches the expected value.
    fn verify_request_id(resp: &PluginIpcResponse, expected: &str) -> Result<(), PluginHostError> {
        let actual = match resp {
            PluginIpcResponse::HandshakeAck { .. } | PluginIpcResponse::Pong => return Ok(()),
            PluginIpcResponse::Ack { request_id }
            | PluginIpcResponse::ConfigSchema { request_id, .. }
            | PluginIpcResponse::Error { request_id, .. }
            | PluginIpcResponse::Tools { request_id, .. }
            | PluginIpcResponse::CallResult { request_id, .. }
            | PluginIpcResponse::DeferredAccepted { request_id, .. }
            | PluginIpcResponse::DeferredStatus { request_id, .. }
            | PluginIpcResponse::StreamChunk { request_id, .. }
            | PluginIpcResponse::StreamEnd { request_id, .. }
            | PluginIpcResponse::StreamError { request_id, .. }
            | PluginIpcResponse::ChatCompletionResult { request_id, .. }
            | PluginIpcResponse::EmbedBatchResult { request_id, .. } => request_id,
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
            | PluginIpcRequest::CreateChatStream { request_id, .. }
            | PluginIpcRequest::ChatCompletion { request_id, .. }
            | PluginIpcRequest::EmbedBatch { request_id, .. } => Some(request_id.as_str()),
            PluginIpcRequest::Handshake { .. }
            | PluginIpcRequest::Shutdown
            | PluginIpcRequest::Ping => None,
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
    /// non-idempotent call. Streaming reads
    /// ([`read_response`](Self::read_response)) bypass this path entirely and
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
        match self.request_once(&req, timeout).await {
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
                // Drop the stale stream so reconnect starts from a clean slate.
                self.stream = None;
                self.reconnect().await?;
                let resp = self.request_once(&req, timeout).await?;
                Self::verify_request_id(&resp, &request_id)?;
                Ok(resp)
            }
            Err(e) => Err(e),
        }
    }

    /// Performs a single request/response round-trip, classifying transport
    /// failures as [`PluginHostError::TransportFailed`] so the caller can
    /// decide whether a reconnect-and-retry is warranted.
    async fn request_once(
        &mut self,
        req: &PluginIpcRequest,
        timeout: Duration,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        self.send_request(req).await?;
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| PluginHostError::transport("not connected to plugin"))?;
        let fut = read_plugin_response(stream);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(Some(resp))) => Ok(resp),
            Ok(Ok(None)) => Err(PluginHostError::transport("connection closed by plugin")),
            Ok(Err(e)) => Err(PluginHostError::transport(format!("read failed: {e}"))),
            Err(_elapsed) => Err(PluginHostError::execution(format!(
                "timed out after {} ms",
                timeout.as_millis()
            ))),
        }
    }

    /// Writes a request to the stream (no read).
    ///
    /// A missing stream or a failed write is reported as
    /// [`PluginHostError::TransportFailed`], which the request path treats as
    /// reconnectable.
    async fn send_request(&mut self, req: &PluginIpcRequest) -> Result<(), PluginHostError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| PluginHostError::transport("not connected to plugin"))?;
        write_plugin_request(stream, req)
            .await
            .map_err(|e| PluginHostError::transport(format!("write failed: {e}")))
    }
}
