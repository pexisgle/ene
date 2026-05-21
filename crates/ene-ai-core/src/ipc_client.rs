use crate::tools::definition::{ToolDefinition, ToolRegistry};
use crate::tools::ToolCategory;
use async_trait::async_trait;
use ene_tool_proto::{
    read_ipc_response, write_ipc_request, IpcRequest, IpcResponse, SandboxConfigData,
    ToolCategory as ProtoCategory, ToolDefinition as ProtoToolDefinition,
};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::net::UnixStream;
use tokio::sync::Mutex as TokioMutex;

/// IPC 経由で `ene-tool-host` と通信する ToolRegistry 実装
///
/// `ene-ai-core` が `ToolRegistry` trait を通じてツールを呼び出す際、
/// UDS (Unix Domain Socket) 経由で別プロセスの `ene-tool-host` に問い合わせる。
#[allow(dead_code)]
pub struct IpcToolRegistry {
    socket_path: PathBuf,
    stream: TokioMutex<Option<UnixStream>>,
    tools: Mutex<Vec<ToolDefinition>>,
}

impl IpcToolRegistry {
    pub async fn new(socket_path: PathBuf, sandbox: SandboxConfigData) -> Result<Self, String> {
        let mut stream = UnixStream::connect(&socket_path)
            .await
            .map_err(|e| format!("Failed to connect to tool host at {socket_path:?}: {e}"))?;

        write_ipc_request(&mut stream, &IpcRequest::Initialize { sandbox })
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
            stream: TokioMutex::new(Some(stream)),
            tools: Mutex::new(Vec::new()),
        };

        // 接続直後にツール一覧を取得してキャッシュ
        registry.refresh_tools().await?;

        Ok(registry)
    }

    /// ツール一覧を host から再取得しキャッシュを更新する
    pub async fn refresh_tools(&self) -> Result<(), String> {
        match self.send_request(IpcRequest::ListTools).await? {
            IpcResponse::Tools { tools } => {
                let converted: Vec<ToolDefinition> = tools.into_iter().map(proto_to_core).collect();
                let mut guard = self.tools.lock().map_err(|e| e.to_string())?;
                *guard = converted;
                Ok(())
            }
            IpcResponse::Error { message } => Err(message),
            _ => Err("Unexpected response for ListTools".to_string()),
        }
    }

    async fn send_request(&self, req: IpcRequest) -> Result<IpcResponse, String> {
        let mut guard = self.stream.lock().await;
        let stream = guard.as_mut().ok_or("Not connected to tool host")?;
        write_ipc_request(stream, &req)
            .await
            .map_err(|e| e.to_string())?;
        let resp = read_ipc_response(stream)
            .await
            .map_err(|e| e.to_string())?;
        resp.ok_or("Connection closed by tool host".to_string())
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

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, String> {
        match self
            .send_request(IpcRequest::CallTool {
                name: name.to_string(),
                arguments: arguments.to_string(),
            })
            .await
        {
            Ok(IpcResponse::CallResult { result }) => result.map_err(|e| e.to_string()),
            Ok(IpcResponse::Error { message }) => Err(message),
            Ok(_) => Err("Unexpected response for CallTool".to_string()),
            Err(e) => Err(e),
        }
    }

    fn set_session_id(&self, session_id: &str) {
        let rt = tokio::runtime::Handle::try_current().ok();
        if let Some(handle) = rt {
            let req = IpcRequest::SetSessionId {
                session_id: session_id.to_string(),
            };
            let _ = handle.block_on(self.send_request(req));
        }
    }

    async fn ensure_index_built(
        &self,
        _embedder: &dyn crate::embedding::EmbeddingProvider,
        _store: Option<&crate::memory::store::MemoryStore>,
    ) -> Result<(), String> {
        // IPC 先の Provider は embedding を知らない。core 側の責務。
        Ok(())
    }
}

fn proto_to_core(proto: ProtoToolDefinition) -> ToolDefinition {
    ToolDefinition {
        name: proto.name,
        description: proto.description,
        parameters: proto.parameters,
        category: proto.category.map(|c| match c {
            ProtoCategory::Filesystem => ToolCategory::Filesystem,
            ProtoCategory::Shell => ToolCategory::Shell,
            ProtoCategory::Browser => ToolCategory::Browser,
            ProtoCategory::App => ToolCategory::App,
            ProtoCategory::WebSearch => ToolCategory::WebSearch,
            ProtoCategory::Utility => ToolCategory::Utility,
        }),
        keywords: proto.keywords,
    }
}
