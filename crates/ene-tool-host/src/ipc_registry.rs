use crate::ToolHostError;
use crate::tools::registry::ToolRegistry;
use async_trait::async_trait;
use ene_tool_proto::transport::IpcStream;
use ene_tool_proto::{
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, SandboxConfigData, ToolRagProfile, ToolSpec,
    read_ipc_response, write_ipc_request,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;

const RECONNECT_MAX_RETRIES: u32 = 5;
const RECONNECT_BASE_DELAY_MS: u64 = 200;
const RECONNECT_MAX_DELAY_MS: u64 = 10_000;
/// Timeout for permission/pattern approval IPC calls (30s).
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout for a `Ping` liveness probe (#238). Kept short so a hung
/// tool is detected quickly without waiting for the full call timeout.
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// A `ToolRegistry` implementation that communicates with tool binaries via IPC
///
/// Automatically retries connection with exponential backoff when disconnected
pub struct IpcToolRegistry {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    tool_config: Option<serde_json::Value>,
    stream: TokioMutex<Option<IpcStream>>,
    tools: Mutex<Vec<ToolSpec>>,
    profiles: Mutex<Vec<ToolRagProfile>>,
    /// Per-call timeout applied to every request sent to the tool
    /// binary. Sourced from `ToolConfig::timeout_ms` (default 60s).
    /// Without this, a tool that hangs inside `execute()` blocks
    /// the host's call indefinitely, and because calls to one tool
    /// are serialized through the `TokioMutex`, all subsequent
    /// calls to that tool binary also block.
    timeout_ms: u64,
}

impl IpcToolRegistry {
    /// Creates a new IPC tool registry client, connecting to the tool binary at the given socket path.
    ///
    /// `timeout_ms` is the per-call request timeout; pass 0 to use
    /// the default of 60 seconds.
    pub async fn new(
        socket_path: PathBuf,
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
        timeout_ms: u64,
    ) -> Result<Self, ToolHostError> {
        let mut stream = Self::connect_with_retry(&socket_path, RECONNECT_MAX_RETRIES).await?;

        // Perform handshake (v3: folds Initialize sandbox+tool_config into a single RT)
        write_ipc_request(
            &mut stream,
            &IpcRequest::Handshake {
                version: IPC_PROTOCOL_VERSION,
                sandbox: sandbox.clone(),
                tool_config: tool_config.clone(),
            },
        )
        .await
        .map_err(|e| ToolHostError::ipc(format!("Failed to send Handshake: {e}")))?;

        let resp = read_ipc_response(&mut stream)
            .await
            .map_err(|e| ToolHostError::ipc(format!("Failed to read Handshake response: {e}")))?;

        match resp {
            Some(IpcResponse::HandshakeAck { version }) => {
                if version != IPC_PROTOCOL_VERSION {
                    return Err(ToolHostError::ipc(format!(
                        "Handshake version mismatch: server acked {version}, client requires {IPC_PROTOCOL_VERSION}"
                    )));
                }
            }
            Some(IpcResponse::Error { message }) => {
                return Err(ToolHostError::ipc(message));
            }
            _ => {
                return Err(ToolHostError::ipc("Unexpected response to Handshake"));
            }
        }

        let registry = Self {
            socket_path,
            sandbox,
            tool_config,
            stream: TokioMutex::new(Some(stream)),
            tools: Mutex::new(Vec::new()),
            profiles: Mutex::new(Vec::new()),
            timeout_ms: if timeout_ms == 0 { 60_000 } else { timeout_ms },
        };

        registry.do_refresh_tools().await?;

        Ok(registry)
    }

    async fn connect_with_retry(
        socket_path: &Path,
        max_retries: u32,
    ) -> Result<IpcStream, ToolHostError> {
        let mut attempts = 0_u32;
        loop {
            match IpcStream::connect(socket_path).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempts = attempts.saturating_add(1);
                    if attempts >= max_retries {
                        return Err(ToolHostError::ipc(format!(
                            "Failed to connect to tool at {} after {} attempts: {e}",
                            socket_path.display(),
                            attempts
                        )));
                    }
                    let delay = RECONNECT_BASE_DELAY_MS
                        .saturating_mul(2u64.saturating_pow(attempts.saturating_sub(1)));
                    tokio::time::sleep(Duration::from_millis(delay.min(RECONNECT_MAX_DELAY_MS)))
                        .await;
                }
            }
        }
    }

    /// Sends an `IpcRequest` and receives an `IpcResponse`. Retries connection once on disconnect.
    ///
    /// The response read is bounded by `self.timeout_ms`. A hung
    /// tool binary (deadlock / infinite loop in `execute()`) will
    /// trip the timeout, the connection is closed, and subsequent
    /// `call_tool` invocations fall through to the supervised
    /// restart path in `SupervisedIpcRegistry` instead of blocking
    /// forever behind the still-locked stream mutex.
    async fn do_request(&self, req: IpcRequest) -> Result<IpcResponse, ToolHostError> {
        self.do_request_with_timeout(req, Duration::from_millis(self.timeout_ms))
            .await
    }

    /// Like [`do_request`](Self::do_request) but with an explicit read
    /// timeout. Used by the liveness probe, which needs a short bound
    /// independent of the (potentially long) per-call timeout (#238).
    async fn do_request_with_timeout(
        &self,
        req: IpcRequest,
        timeout: Duration,
    ) -> Result<IpcResponse, ToolHostError> {
        let result = {
            let mut guard = self.stream.lock().await;
            let Some(stream) = guard.as_mut() else {
                return Err(ToolHostError::ipc("Not connected to tool host"));
            };

            if let Err(e) = write_ipc_request(stream, &req).await {
                drop(guard);
                return Err(ToolHostError::ipc(format!("Failed to send request: {e}")));
            }

            let read_fut = read_ipc_response(stream);
            match tokio::time::timeout(timeout, read_fut).await {
                Ok(Ok(Some(resp))) => Ok(resp),
                Ok(Ok(None)) => Err(ToolHostError::ipc("Connection closed by tool host")),
                Ok(Err(e)) => Err(ToolHostError::ipc(format!("Failed to read response: {e}"))),
                Err(_elapsed) => Err(ToolHostError::ipc(format!(
                    "Tool call timed out after {} ms",
                    timeout.as_millis()
                ))),
            }
        };

        if result.is_err() {
            let mut guard = self.stream.lock().await;
            *guard = None;
        }

        result
    }

    /// Sends a `Ping` liveness probe and waits for a `Pong` (#238).
    ///
    /// Returns `Ok(())` when the tool answers within the probe bound (the
    /// smaller of [`PING_TIMEOUT`] and the configured per-call timeout).
    /// A timeout or transport error means the tool is alive at the OS
    /// level but unresponsive (hung), which the supervisor treats as
    /// unhealthy and restarts.
    pub async fn ping(&self) -> Result<(), ToolHostError> {
        let probe_timeout = Duration::from_millis(self.timeout_ms).min(PING_TIMEOUT);
        match self
            .do_request_with_timeout(IpcRequest::Ping, probe_timeout)
            .await?
        {
            IpcResponse::Pong => Ok(()),
            IpcResponse::Error { message } => Err(ToolHostError::ipc(message)),
            other => Err(ToolHostError::ipc(format!(
                "Unexpected response to Ping: {other:?}"
            ))),
        }
    }

    async fn do_refresh_tools_with_stream(
        &self,
        stream: &mut IpcStream,
    ) -> Result<(), ToolHostError> {
        write_ipc_request(stream, &IpcRequest::ListTools)
            .await
            .map_err(|e| ToolHostError::ipc(format!("Failed to send ListTools: {e}")))?;

        let resp = read_ipc_response(stream)
            .await
            .map_err(|e| ToolHostError::ipc(format!("Failed to read ListTools response: {e}")))?;

        match resp {
            Some(IpcResponse::Tools { tools }) => {
                let mut tools_guard = self
                    .tools
                    .lock()
                    .map_err(|e| ToolHostError::ipc(format!("Tools lock poisoned: {e}")))?;
                *tools_guard = tools;
                drop(tools_guard);
            }
            Some(IpcResponse::Error { message }) => return Err(ToolHostError::ipc(message)),
            _ => return Err(ToolHostError::ipc("Unexpected response for ListTools")),
        }

        write_ipc_request(stream, &IpcRequest::ListRagProfiles)
            .await
            .map_err(|e| ToolHostError::ipc(format!("Failed to send ListRagProfiles: {e}")))?;

        let resp = read_ipc_response(stream).await.map_err(|e| {
            ToolHostError::ipc(format!("Failed to read ListRagProfiles response: {e}"))
        })?;

        match resp {
            Some(IpcResponse::RagProfiles { profiles }) => {
                let mut profiles_guard = self
                    .profiles
                    .lock()
                    .map_err(|e| ToolHostError::ipc(format!("Profiles lock poisoned: {e}")))?;
                *profiles_guard = profiles;
                drop(profiles_guard);
                Ok(())
            }
            Some(IpcResponse::Error { message }) => Err(ToolHostError::ipc(message)),
            _ => Err(ToolHostError::ipc(
                "Unexpected response for ListRagProfiles",
            )),
        }
    }

    async fn do_refresh_tools(&self) -> Result<(), ToolHostError> {
        let mut guard = self.stream.lock().await;
        let _ = self
            .do_refresh_tools_with_stream(
                guard
                    .as_mut()
                    .ok_or_else(|| ToolHostError::ipc("Not connected"))?,
            )
            .await;
        drop(guard);
        Ok(())
    }

    /// Refreshes the cached tool definitions from the tool binary.
    pub async fn refresh_tools(&self) -> Result<(), ToolHostError> {
        self.do_refresh_tools().await
    }

    /// Returns the socket path this client is connected to.
    pub const fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    /// Retrieves the configuration JSON schema from the tool binary.
    pub async fn get_config_schema(&self) -> Option<serde_json::Value> {
        match self.send_with_reconnect(IpcRequest::GetConfigSchema).await {
            Ok(IpcResponse::ConfigSchema { schema }) => schema,
            _ => None,
        }
    }

    /// Attempts reconnection when disconnected
    async fn ensure_connected(&self) -> Result<(), ToolHostError> {
        {
            let guard = self.stream.lock().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        tracing::warn!(
            "[IpcToolRegistry] Connection lost, reconnecting to {}",
            self.socket_path.display()
        );

        let mut stream = Self::connect_with_retry(&self.socket_path, RECONNECT_MAX_RETRIES).await?;

        // Re-handshake (v3: folds Initialize into single RT)
        write_ipc_request(
            &mut stream,
            &IpcRequest::Handshake {
                version: IPC_PROTOCOL_VERSION,
                sandbox: self.sandbox.clone(),
                tool_config: self.tool_config.clone(),
            },
        )
        .await
        .map_err(|e| ToolHostError::ipc(format!("Failed to send Handshake on reconnect: {e}")))?;

        let resp = read_ipc_response(&mut stream).await.map_err(|e| {
            ToolHostError::ipc(format!(
                "Failed to read Handshake response on reconnect: {e}"
            ))
        })?;

        match resp {
            Some(IpcResponse::HandshakeAck { .. }) => {}
            Some(IpcResponse::Error { message }) => {
                return Err(ToolHostError::ipc(format!(
                    "Reconnect handshake rejected: {message}"
                )));
            }
            _ => {
                return Err(ToolHostError::ipc(
                    "Unexpected response to Handshake on reconnect",
                ));
            }
        }

        self.do_refresh_tools_with_stream(&mut stream).await?;

        {
            let mut guard = self.stream.lock().await;
            *guard = Some(stream);
        }

        tracing::info!(
            "[IpcToolRegistry] Successfully reconnected to {}",
            self.socket_path.display()
        );
        Ok(())
    }

    async fn send_with_reconnect(&self, req: IpcRequest) -> Result<IpcResponse, ToolHostError> {
        let result = self.do_request(req.clone()).await;

        if let Ok(resp) = result {
            Ok(resp)
        } else {
            self.ensure_connected().await?;
            self.do_request(req).await
        }
    }
}

#[async_trait]
impl ToolRegistry for IpcToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        match self.tools.lock() {
            Ok(guard) => guard.clone(),
            Err(e) => {
                tracing::warn!("[IpcToolRegistry] Failed to lock tools cache: {e}");
                Vec::new()
            }
        }
    }

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        match self.profiles.lock() {
            Ok(guard) => {
                if guard.is_empty() {
                    // Fallback: synthesize from tools if profiles were never
                    // refreshed (should not happen after a successful handshake).
                    drop(guard);
                    return self
                        .list_tools()
                        .iter()
                        .map(ToolRagProfile::from_tool_spec)
                        .collect();
                }
                guard.clone()
            }
            Err(e) => {
                tracing::warn!("[IpcToolRegistry] Failed to lock profiles cache: {e}");
                Vec::new()
            }
        }
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        self.send_with_reconnect(IpcRequest::CallTool {
            name: name.to_string(),
            arguments: arguments.to_string(),
        })
        .await
        .and_then(|resp| match resp {
            IpcResponse::CallResult { result } => result.map_err(Into::into),
            IpcResponse::Error { message } => Err(ToolHostError::ExecutionFailed { message }),
            _ => Err(ToolHostError::ExecutionFailed {
                message: "Unexpected response for CallTool".to_string(),
            }),
        })
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        self.get_config_schema().await
    }

    async fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        let req = IpcRequest::SetCallContext {
            conversation_id: ctx.conversation_id.clone(),
            turn_id: ctx.turn_id.clone(),
        };
        let _ = self.send_with_reconnect(req).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        let req = IpcRequest::ApprovePermission {
            request_id: request_id.to_string(),
        };
        let send = self.send_with_reconnect(req);
        let _ = tokio::time::timeout(PERMISSION_TIMEOUT, send).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let req = IpcRequest::AllowPattern {
            action: action.to_string(),
            target_pattern: target_pattern.to_string(),
        };
        let send = self.send_with_reconnect(req);
        let _ = tokio::time::timeout(PERMISSION_TIMEOUT, send).await;
    }

    async fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        let req = IpcRequest::RevokePattern {
            action: action.to_string(),
            target_pattern: target_pattern.to_string(),
        };
        let send = self.send_with_reconnect(req);
        let _ = tokio::time::timeout(PERMISSION_TIMEOUT, send).await;
    }
}
