use crate::error::ToolError;
use crate::sandbox::SandboxConfigData;
use crate::types::ToolDefinition;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// IPC リクエスト — core → host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcRequest {
    Initialize { sandbox: SandboxConfigData },
    ListTools,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    Ping,
    Shutdown,
}

/// IPC レスポンス — host → core
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcResponse {
    Ack,
    Tools { tools: Vec<ToolDefinition> },
    CallResult { result: Result<String, ToolError> },
    Pong,
    Error { message: String },
}

/// 4バイト長前置き + JSON でIpcRequestを読み込む
///
/// UnexpectedEof の場合は `Ok(None)` を返し、接続終了を表す。
pub async fn read_ipc_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcRequest>, ToolError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(ToolError::from)?;
    let req = serde_json::from_slice(&buf).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to deserialize IpcRequest: {e}"),
    })?;
    Ok(Some(req))
}

/// 4バイト長前置き + JSON でIpcRequestを書き込む
pub async fn write_ipc_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    req: &IpcRequest,
) -> Result<(), ToolError> {
    let json = serde_json::to_vec(req).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to serialize IpcRequest: {e}"),
    })?;
    let len = json.len() as u32;
    writer
        .write_all(&len.to_le_bytes())
        .await
        .map_err(ToolError::from)?;
    writer.write_all(&json).await.map_err(ToolError::from)?;
    writer.flush().await.map_err(ToolError::from)?;
    Ok(())
}

/// 4バイト長前置き + JSON でIpcResponseを読み込む
///
/// UnexpectedEof の場合は `Ok(None)` を返し、接続終了を表す。
pub async fn read_ipc_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcResponse>, ToolError> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 {
        return Ok(None);
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(ToolError::from)?;
    let resp = serde_json::from_slice(&buf).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to deserialize IpcResponse: {e}"),
    })?;
    Ok(Some(resp))
}

/// 4バイト長前置き + JSON でIpcResponseを書き込む
pub async fn write_ipc_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    resp: &IpcResponse,
) -> Result<(), ToolError> {
    let json = serde_json::to_vec(resp).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to serialize IpcResponse: {e}"),
    })?;
    let len = json.len() as u32;
    writer
        .write_all(&len.to_le_bytes())
        .await
        .map_err(ToolError::from)?;
    writer.write_all(&json).await.map_err(ToolError::from)?;
    writer.flush().await.map_err(ToolError::from)?;
    Ok(())
}
