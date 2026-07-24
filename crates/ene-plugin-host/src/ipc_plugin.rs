//! IPC client to a single plugin binary.
//!
//! [`IpcPluginConnection`] manages the lifecycle of one connection to a
//! plugin process: handshake, request/response, and reconnection on failure.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ene_plugin_proto::SandboxConfigData;
use ene_plugin_proto::{
    IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
    PluginIpcResponse, read_plugin_response, write_plugin_request,
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
                version: PLUGIN_IPC_PROTOCOL_VERSION,
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
            Some(PluginIpcResponse::Error { message }) => {
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

    /// Sends a `Ping` and waits for `Pong` within [`PING_TIMEOUT`].
    pub async fn ping(&mut self) -> Result<(), PluginHostError> {
        let resp = self
            .do_request_with_timeout(PluginIpcRequest::Ping, PING_TIMEOUT)
            .await?;
        match resp {
            PluginIpcResponse::Pong => Ok(()),
            PluginIpcResponse::Error { message } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to Ping: {other:?}"
            ))),
        }
    }

    /// Calls a tool exposed by the plugin and returns the result string.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: &str,
    ) -> Result<String, PluginHostError> {
        let resp = self
            .do_request(PluginIpcRequest::CallTool {
                name: name.to_string(),
                arguments: arguments.to_string(),
                deferred: false,
            })
            .await?;
        match resp {
            PluginIpcResponse::CallResult { result } => {
                result.map_err(|e| PluginHostError::execution(e.to_string()))
            }
            PluginIpcResponse::Error { message } => Err(PluginHostError::execution(message)),
            other => Err(PluginHostError::execution(format!(
                "unexpected response to CallTool: {other:?}"
            ))),
        }
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
            PluginIpcResponse::Error { message } => Err(PluginHostError::execution(message)),
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
                version: PLUGIN_IPC_PROTOCOL_VERSION,
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
            Some(PluginIpcResponse::Error { message }) => {
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

    /// Sends a request and reads a single response with an explicit timeout.
    async fn do_request_with_timeout(
        &mut self,
        req: PluginIpcRequest,
        timeout: Duration,
    ) -> Result<PluginIpcResponse, PluginHostError> {
        self.send_request(&req).await?;
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| PluginHostError::execution("not connected to plugin"))?;
        let fut = read_plugin_response(stream);
        match tokio::time::timeout(timeout, fut).await {
            Ok(Ok(Some(resp))) => Ok(resp),
            Ok(Ok(None)) => Err(PluginHostError::execution("connection closed by plugin")),
            Ok(Err(e)) => Err(PluginHostError::execution(format!("read failed: {e}"))),
            Err(_elapsed) => Err(PluginHostError::execution(format!(
                "timed out after {} ms",
                timeout.as_millis()
            ))),
        }
    }

    /// Writes a request to the stream (no read).
    async fn send_request(&mut self, req: &PluginIpcRequest) -> Result<(), PluginHostError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| PluginHostError::execution("not connected to plugin"))?;
        write_plugin_request(stream, req)
            .await
            .map_err(|e| PluginHostError::execution(format!("write failed: {e}")))
    }
}
