pub mod error;
pub mod ipc;
pub mod sandbox;
pub mod types;

pub use error::ToolError;
pub use ipc::{
    IpcRequest, IpcResponse, read_ipc_request, read_ipc_response, write_ipc_request,
    write_ipc_response,
};
pub use sandbox::SandboxConfigData;
pub use types::{ToolCallResult, ToolCategory, ToolDefinition};

use async_trait::async_trait;

/// ツールプロバイダ trait — 各ツールクレートが実装する
///
/// IPC分離後、host側の各Providerがこのtraitを実装し、
/// core側のIpcToolRegistryがIPC経由で呼び出す。
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// 提供するツール一覧を返す
    fn list_tools(&self) -> Vec<ToolDefinition>;

    /// ツールを実行する
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// 現在のセッションIDを設定（Undo等で使用）
    fn set_session_id(&self, session_id: &str);
}
