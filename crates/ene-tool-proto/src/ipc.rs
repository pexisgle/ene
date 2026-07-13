use crate::error::ToolError;
use crate::sandbox::SandboxConfigData;
use crate::types::{ActionSpec, ToolSpec};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum allowed IPC message size in bytes (64 MB).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Current IPC protocol version.
///
/// Version 1 (reset):
/// - `IpcResponse::Tools` carries `Vec<ToolSpec>`
/// - `IpcResponse::ActionSpecs` returns per-action metadata for embedding
/// - `IpcRequest::CallTool` `name` field accepts the new `ToolName` (still
///   a string on the wire)
/// - `SandboxConfigData::db_socket` replaces old `db_path`
pub const IPC_PROTOCOL_VERSION: u32 = 2;

/// IPC request — core → host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcRequest {
    /// Handshake to negotiate protocol version.
    Handshake {
        /// Client's supported protocol version.
        version: u32,
    },
    /// Initialise the tool with sandbox and config data.
    Initialize {
        /// Sandbox configuration to apply.
        sandbox: SandboxConfigData,
        /// Tool-specific configuration JSON.
        tool_config: Option<serde_json::Value>,
    },
    /// List all available tool specs.
    ListTools,
    /// List per-action specs (mega-tool capability metadata).
    ListActionSpecs,
    /// Request the tool's config JSON Schema.
    GetConfigSchema,
    /// Execute a tool by name with JSON arguments.
    CallTool {
        /// Tool name to call.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// Set the current session ID.
    SetSessionId {
        /// Session identifier.
        session_id: String,
    },
    /// Approve a pending permission request.
    ApprovePermission {
        /// ID of the request to approve.
        request_id: String,
    },
    /// Register a session-wide permission allow pattern.
    AllowPattern {
        /// Action pattern (e.g. "`filesystem_write`").
        action: String,
        /// Target glob pattern.
        target_pattern: String,
    },
    /// Get tool configuration.
    GetMyConfig,
    /// Set tool configuration.
    SetMyConfig(serde_json::Value),
    /// Health-check ping.
    Ping,
    /// Graceful shutdown.
    Shutdown,
}

/// IPC response — host → core
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcResponse {
    /// Handshake acknowledgment with negotiated version.
    HandshakeAck {
        /// Agreed protocol version.
        version: u32,
    },
    /// Acknowledgment (for Initialize, `SetSessionId`, etc.).
    Ack,
    /// List of tool specs (v2).
    Tools {
        /// The structured tool specs.
        tools: Vec<ToolSpec>,
    },
    /// Per-action specs (v2). For mega-tools, one entry per action.
    ActionSpecs {
        /// The action specs.
        specs: Vec<ActionSpec>,
    },
    /// The tool's config JSON Schema.
    ConfigSchema {
        /// The schema, or None if not provided.
        schema: Option<serde_json::Value>,
    },
    /// Result of a tool call.
    CallResult {
        /// The result, or an error.
        result: Result<String, ToolError>,
    },
    /// Tool configuration.
    MyConfig(serde_json::Value),
    /// Pong response to Ping.
    Pong,
    /// Error response.
    Error {
        /// Error description.
        message: String,
    },
}

/// Accessor for tool configuration.
pub struct ToolConfigAccessor {
    config: std::sync::Arc<tokio::sync::RwLock<serde_json::Value>>,
}

impl ToolConfigAccessor {
    /// Create a new accessor.
    pub fn new(initial_config: serde_json::Value) -> Self {
        Self {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(initial_config)),
        }
    }

    /// Gets the tool configuration.
    ///
    /// Returns a [`ToolError::InvalidArguments`] when the stored JSON
    /// does not deserialize into `T` — the previous implementation
    /// returned `T::default()` on a deserialize failure, which silently
    /// masked config corruption and confused callers into thinking the
    /// default was a real value. A typed error gives the host a chance
    /// to log the bad payload and fail the request.
    pub async fn get<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolError> {
        let guard = self.config.read().await;
        serde_json::from_value(guard.clone()).map_err(|e| ToolError::InvalidArguments {
            message: format!("Stored tool config does not match expected type: {e}"),
        })
    }

    /// Sets the tool configuration.
    pub async fn set<T: serde::Serialize>(&self, config: &T) -> Result<(), ToolError> {
        let val = serde_json::to_value(config).map_err(|e| ToolError::InvalidArguments {
            message: format!("Failed to serialize config: {e}"),
        })?;
        let mut guard = self.config.write().await;
        *guard = val;
        Ok(())
    }
}

/// Reads an `IpcRequest` as 4-byte length-prefixed JSON
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed
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
    if len > MAX_MESSAGE_SIZE {
        return Err(ToolError::IpcTransport {
            message: format!("Request size {len} exceeds maximum {MAX_MESSAGE_SIZE}"),
        });
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(ToolError::from)?;
    let req = serde_json::from_slice(&buf).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to deserialize IpcRequest: {e}"),
    })?;
    Ok(Some(req))
}

/// Writes an `IpcRequest` as 4-byte length-prefixed JSON
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

/// Reads an `IpcResponse` as 4-byte length-prefixed JSON
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed
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
    if len > MAX_MESSAGE_SIZE {
        return Err(ToolError::IpcTransport {
            message: format!("Response size {len} exceeds maximum {MAX_MESSAGE_SIZE}"),
        });
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(ToolError::from)?;
    let resp = serde_json::from_slice(&buf).map_err(|e| ToolError::InvalidArguments {
        message: format!("Failed to deserialize IpcResponse: {e}"),
    })?;
    Ok(Some(resp))
}

/// Writes an `IpcResponse` as 4-byte length-prefixed JSON
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
    use crate::{ActionSpec, SandboxConfigData, ToolName, ToolSpec};

    async fn send_recv_request(req: &IpcRequest) -> IpcRequest {
        let (mut a, mut b) = tokio::io::duplex(4096);
        write_ipc_request(&mut a, req).await.unwrap();
        drop(a);
        read_ipc_request(&mut b).await.unwrap().unwrap()
    }

    async fn send_recv_response(resp: &IpcResponse) -> IpcResponse {
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
            db_socket: None,
            db_auth_token: None,
        };
        let req = IpcRequest::Initialize {
            sandbox: sandbox.clone(),
            tool_config: None,
        };
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
    async fn ipc_request_handshake_v2_roundtrip() {
        let req = IpcRequest::Handshake {
            version: IPC_PROTOCOL_VERSION,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_list_action_specs_roundtrip() {
        let req = IpcRequest::ListActionSpecs;
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
        let tools = vec![ToolSpec::new(
            ToolName::new("test"),
            "desc",
            serde_json::json!({}),
        )];
        let resp = IpcResponse::Tools { tools };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_response_action_specs_roundtrip() {
        let specs = vec![ActionSpec::minimal("read", "Read a file")];
        let resp = IpcResponse::ActionSpecs { specs };
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
