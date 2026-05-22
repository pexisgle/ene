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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SandboxConfigData, ToolCategory};

    async fn send_recv_request(
        req: &IpcRequest,
    ) -> IpcRequest {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_ipc_request(&mut a, req).await.unwrap();
        drop(a);
        read_ipc_request(&mut b).await.unwrap().unwrap()
    }

    async fn send_recv_response(
        resp: &IpcResponse,
    ) -> IpcResponse {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_ipc_response(&mut a, resp).await.unwrap();
        drop(a);
        read_ipc_response(&mut b).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn ipc_request_initialize_roundtrip() {
        let sandbox = SandboxConfigData {
            enabled: true,
            allowed_directories: vec![],
            writable_directories: vec![],
            blocked_commands: vec![],
            max_read_bytes: 0,
            max_write_bytes: 0,
            shell_timeout_ms: 0,
            max_shell_output_bytes: 0,
            max_shell_output_lines: 0,
            undo_db_path: None,
        };
        let req = IpcRequest::Initialize { sandbox: sandbox.clone() };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_list_tools_roundtrip() {
        let req = IpcRequest::ListTools;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_call_tool_roundtrip() {
        let req = IpcRequest::CallTool {
            name: "read".into(),
            arguments: r#"{"path":"/tmp/test.txt"}"#.into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_set_session_id_roundtrip() {
        let req = IpcRequest::SetSessionId {
            session_id: "sess_abc123".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_ping_roundtrip() {
        let req = IpcRequest::Ping;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_shutdown_roundtrip() {
        let req = IpcRequest::Shutdown;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_response_ack_roundtrip() {
        let resp = IpcResponse::Ack;
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_tools_roundtrip() {
        let tools = vec![ToolDefinition {
            name: "test".into(),
            description: "desc".into(),
            parameters: serde_json::json!({}),
            category: Some(ToolCategory::Filesystem),
            keywords: vec![],
        }];
        let resp = IpcResponse::Tools { tools };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_call_result_roundtrip() {
        let resp = IpcResponse::CallResult {
            result: Ok("success".into()),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_call_result_error_roundtrip() {
        let resp = IpcResponse::CallResult {
            result: Err(ToolError::NotFound {
                tool_name: "foo".into(),
            }),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_pong_roundtrip() {
        let resp = IpcResponse::Pong;
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_error_roundtrip() {
        let resp = IpcResponse::Error {
            message: "something went wrong".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_read_request_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_ipc_request(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn ipc_read_response_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_ipc_response(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn ipc_zero_length_request_returns_none() {
        let mut buf: &[u8] = &[0u8; 4];
        let result = read_ipc_request(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn ipc_large_payload_roundtrip() {
        let big_content = "x".repeat(10_000);
        let resp = IpcResponse::CallResult {
            result: Ok(big_content.clone()),
        };
        let (mut a, mut b) = tokio::io::duplex(64_000);
        write_ipc_response(&mut a, &resp).await.unwrap();
        drop(a);
        let got = read_ipc_response(&mut b).await.unwrap().unwrap();
        match got {
            IpcResponse::CallResult { result } => assert_eq!(result.unwrap(), big_content),
            _ => panic!("Expected CallResult"),
        }
    }
}
