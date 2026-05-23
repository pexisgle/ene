use crate::tools::ToolDefinition;
use crate::tools::definition::ToolRegistry;
use async_trait::async_trait;
use ene_tool_proto::{
    IpcRequest, IpcResponse, SandboxConfigData, read_ipc_response, write_ipc_request,
};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::sync::Mutex as TokioMutex;

const RECONNECT_MAX_RETRIES: u32 = 5;
const RECONNECT_BASE_DELAY_MS: u64 = 200;
const RECONNECT_MAX_DELAY_MS: u64 = 10_000;

/// IPC 経由でツールバイナリと通信する ToolRegistry 実装
///
/// 接続が切れた場合は指数バックオフで自動再接続を試みる。
pub struct IpcToolRegistry {
    socket_path: PathBuf,
    sandbox: SandboxConfigData,
    tool_config: Option<serde_json::Value>,
    stream: TokioMutex<Option<UnixStream>>,
    tools: Mutex<Vec<ToolDefinition>>,
}

impl IpcToolRegistry {
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
    ) -> Result<UnixStream, String> {
        let mut attempts = 0;
        loop {
            match UnixStream::connect(socket_path).await {
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

    /// IpcRequest を送信し、IpcResponse を受信する。接続断時は再接続を1回試みる。
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

    async fn do_refresh_tools_with_stream(&self, stream: &mut UnixStream) -> Result<(), String> {
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

    pub async fn refresh_tools(&self) -> Result<(), String> {
        self.do_refresh_tools().await
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    /// 接続断時に再接続を試みる
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
            IpcResponse::CallResult { result } => {
                result.map_err(|e| crate::error::ToolError::ToolExecutionError(e.to_string()))
            }
            IpcResponse::Error { message } => {
                Err(crate::error::ToolError::ToolExecutionError(message))
            }
            _ => Err(crate::error::ToolError::ToolExecutionError(
                "Unexpected response for CallTool".to_string(),
            )),
        })
    }

    async fn set_session_id(&self, session_id: &str) {
        let req = IpcRequest::SetSessionId {
            session_id: session_id.to_string(),
        };
        let _ = self.send_with_reconnect(req).await;
    }

    async fn ensure_index_built(
        &self,
        _embedder: &dyn ene_embedding::EmbeddingProvider,
        _store: Option<&ene_memory::MemoryStore>,
    ) -> Result<(), crate::error::ToolError> {
        Ok(())
    }
}
