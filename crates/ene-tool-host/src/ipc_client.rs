use crate::tools::ToolDefinition;
use crate::tools::definition::ToolRegistry;
use async_trait::async_trait;
use ene_tool_proto::transport::IpcStream;
use ene_tool_proto::{
    IpcRequest, IpcResponse, SandboxConfigData, read_ipc_response, write_ipc_request,
};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;

const RECONNECT_MAX_RETRIES: u32 = 5;
const RECONNECT_BASE_DELAY_MS: u64 = 200;
const RECONNECT_MAX_DELAY_MS: u64 = 10_000;

/// A ToolRegistry implementation that communicates with tool binaries via IPC
///
/// Automatically retries connection with exponential backoff when disconnected
pub struct IpcToolRegistry {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    tool_config: Option<serde_json::Value>,
    stream: TokioMutex<Option<IpcStream>>,
    tools: Mutex<Vec<ToolDefinition>>,
}

impl IpcToolRegistry {
    /// Creates a new IPC tool registry client, connecting to the tool binary at the given socket path.
    pub async fn new(
        socket_path: PathBuf,
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    ) -> Result<Self, String> {
        let mut stream = Self::connect_with_retry(&socket_path, RECONNECT_MAX_RETRIES).await?;

        write_ipc_request(
            &mut stream,
            &IpcRequest::Initialize {
                sandbox: sandbox.clone(),
                tool_config: tool_config.clone(),
            },
        )
        .await
        .map_err(|e| format!("Failed to send Initialize: {e}"))?;

        let resp = read_ipc_response(&mut stream)
            .await
            .map_err(|e| format!("Failed to read Initialize response: {e}"))?;

        match resp {
            Some(IpcResponse::Ack) => {}
            Some(IpcResponse::Error { message }) => return Err(message),
            _ => return Err("Unexpected response to Initialize".to_string()),
        }

        let registry = Self {
            socket_path,
            sandbox,
            tool_config,
            stream: TokioMutex::new(Some(stream)),
            tools: Mutex::new(Vec::new()),
        };

        registry.do_refresh_tools().await?;

        Ok(registry)
    }

    async fn connect_with_retry(
        socket_path: &PathBuf,
        max_retries: u32,
    ) -> Result<IpcStream, String> {
        let mut attempts = 0;
        loop {
            match IpcStream::connect(socket_path).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        return Err(format!(
                            "Failed to connect to tool at {} after {} attempts: {e}",
                            socket_path.display(),
                            attempts
                        ));
                    }
                    let delay =
                        RECONNECT_BASE_DELAY_MS * 2u64.saturating_pow(attempts.saturating_sub(1));
                    tokio::time::sleep(Duration::from_millis(delay.min(RECONNECT_MAX_DELAY_MS)))
                        .await;
                }
            }
        }
    }

    /// Sends an IpcRequest and receives an IpcResponse. Retries connection once on disconnect
    async fn do_request(&self, req: IpcRequest) -> Result<IpcResponse, String> {
        let result = {
            let mut guard = self.stream.lock().await;
            let stream = match guard.as_mut() {
                Some(s) => s,
                None => return Err("Not connected to tool host".to_string()),
            };

            if let Err(e) = write_ipc_request(stream, &req).await {
                drop(guard);
                return Err(format!("Failed to send request: {e}"));
            }

            match read_ipc_response(stream).await {
                Ok(Some(resp)) => Ok(resp),
                Ok(None) => Err("Connection closed by tool host".to_string()),
                Err(e) => Err(format!("Failed to read response: {e}")),
            }
        };

        if result.is_err() {
            let mut guard = self.stream.lock().await;
            *guard = None;
        }

        result
    }

    async fn do_refresh_tools_with_stream(&self, stream: &mut IpcStream) -> Result<(), String> {
        write_ipc_request(stream, &IpcRequest::ListTools)
            .await
            .map_err(|e| format!("Failed to send ListTools: {e}"))?;

        let resp = read_ipc_response(stream)
            .await
            .map_err(|e| format!("Failed to read ListTools response: {e}"))?;

        match resp {
            Some(IpcResponse::Tools { tools }) => {
                let mut tools_guard = self.tools.lock().map_err(|e| e.to_string())?;
                *tools_guard = tools;
                Ok(())
            }
            Some(IpcResponse::Error { message }) => Err(message),
            _ => Err("Unexpected response for ListTools".to_string()),
        }
    }

    async fn do_refresh_tools(&self) -> Result<(), String> {
        let mut guard = self.stream.lock().await;
        let stream = guard.as_mut().ok_or("Not connected")?;
        self.do_refresh_tools_with_stream(stream).await
    }

    /// Refreshes the cached tool definitions from the tool binary.
    pub async fn refresh_tools(&self) -> Result<(), String> {
        self.do_refresh_tools().await
    }

    /// Returns the socket path this client is connected to.
    pub fn socket_path(&self) -> &PathBuf {
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
    async fn ensure_connected(&self) -> Result<(), String> {
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

        write_ipc_request(
            &mut stream,
            &IpcRequest::Initialize {
                sandbox: self.sandbox.clone(),
                tool_config: self.tool_config.clone(),
            },
        )
        .await
        .map_err(|e| format!("Failed to send Initialize on reconnect: {e}"))?;

        let resp = read_ipc_response(&mut stream)
            .await
            .map_err(|e| format!("Failed to read Initialize response on reconnect: {e}"))?;

        match resp {
            Some(IpcResponse::Ack) => {}
            Some(IpcResponse::Error { message }) => {
                return Err(format!("Reconnect rejected: {message}"));
            }
            _ => return Err("Unexpected response to Initialize on reconnect".to_string()),
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

    async fn send_with_reconnect(&self, req: IpcRequest) -> Result<IpcResponse, String> {
        let result = self.do_request(req.clone()).await;

        match result {
            Ok(resp) => Ok(resp),
            Err(_) => {
                self.ensure_connected().await?;
                self.do_request(req).await
            }
        }
    }
}

#[async_trait]
impl ToolRegistry for IpcToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        match self.tools.lock() {
            Ok(guard) => guard.clone(),
            Err(e) => {
                tracing::warn!("[IpcToolRegistry] Failed to lock tools cache: {e}");
                Vec::new()
            }
        }
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, crate::error::ToolError> {
        self.send_with_reconnect(IpcRequest::CallTool {
            name: name.to_string(),
            arguments: arguments.to_string(),
        })
        .await
        .map_err(crate::error::ToolError::ToolExecutionError)
        .and_then(|resp| match resp {
            IpcResponse::CallResult { result } => result.map_err(|e| match e {
                ene_tool_proto::ToolError::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                } => crate::error::ToolError::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                },
                ene_tool_proto::ToolError::PermissionDenied { message } => {
                    crate::error::ToolError::PermissionDenied(message)
                }
                ene_tool_proto::ToolError::SandboxViolation { message } => {
                    crate::error::ToolError::SandboxViolation(message)
                }
                other => crate::error::ToolError::ToolExecutionError(other.to_string()),
            }),
            IpcResponse::Error { message } => {
                Err(crate::error::ToolError::ToolExecutionError(message))
            }
            _ => Err(crate::error::ToolError::ToolExecutionError(
                "Unexpected response for CallTool".to_string(),
            )),
        })
    }

    async fn config_schema(&self) -> Option<serde_json::Value> {
        self.get_config_schema().await
    }

    async fn set_session_id(&self, session_id: &str) {
        let req = IpcRequest::SetSessionId {
            session_id: session_id.to_string(),
        };
        let _ = self.send_with_reconnect(req).await;
    }

    async fn approve_permission(&self, request_id: &str) {
        let req = IpcRequest::ApprovePermission {
            request_id: request_id.to_string(),
        };
        let _ = self.send_with_reconnect(req).await;
    }

    async fn allow_pattern(&self, action: &str, target_pattern: &str) {
        let req = IpcRequest::AllowPattern {
            action: action.to_string(),
            target_pattern: target_pattern.to_string(),
        };
        let _ = self.send_with_reconnect(req).await;
    }

    async fn ensure_index_built(
        &self,
        _embedder: &dyn ene_provider::EmbeddingProvider,
        _store: Option<&ene_memory::MemoryStore>,
    ) -> Result<(), crate::error::ToolError> {
        Ok(())
    }
}
