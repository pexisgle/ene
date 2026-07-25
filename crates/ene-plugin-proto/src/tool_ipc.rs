use crate::sandbox::SandboxConfigData;
use crate::tool_error::ToolError;
use crate::tool_types::{ToolRagProfile, ToolSpec};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum allowed IPC message size in bytes (64 MB).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Current IPC protocol version.
///
/// v2 adds the `Ping`/`Pong` liveness probe used by the host's periodic
/// health check and hang detection (#238), and the deferred-call
/// messages (`CallTool.deferred` / `DeferredAccepted`) that power
/// background tool execution (#196).
pub const IPC_PROTOCOL_VERSION: u32 = 2;

/// IPC request — core → host
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcRequest {
    /// Handshake to negotiate protocol version, exchange sandbox config,
    /// and push tool-specific configuration.
    Handshake {
        /// Client's supported protocol version.
        version: u32,
        /// Sandbox configuration to apply.
        #[serde(default)]
        sandbox: SandboxConfigData,
        /// Tool-specific configuration JSON.
        tool_config: Option<serde_json::Value>,
    },
    /// List all available tool specs.
    ListTools,
    /// List host/RAG metadata profiles for indexed tools (#137).
    ListRagProfiles,
    /// Request the tool's config JSON Schema (documented exception, #150).
    GetConfigSchema,
    /// Execute a tool by name with JSON arguments.
    CallTool {
        /// Tool name to call.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
        /// When `true`, request deferred (background) execution (#196).
        ///
        /// A background-capable tool should start the work asynchronously
        /// and reply with [`IpcResponse::DeferredAccepted`] carrying a
        /// `task_id` instead of blocking on the result. Tools that do not
        /// support deferral ignore this flag and return a normal
        /// [`IpcResponse::CallResult`].
        #[serde(default)]
        deferred: bool,
        /// Per-call context (conversation + turn identifiers).
        ///
        /// Supersedes the deprecated `SetCallContext` message. When present,
        /// the context applies to this single tool call only and does not
        /// persist for subsequent calls.
        #[serde(default)]
        context: Option<CallContext>,
    },
    /// Set the call context (conversation + turn identifiers).
    ///
    /// Supersedes removed `SetSessionId`; tool-side session scoping should
    /// derive from `conversation_id`.
    SetCallContext {
        /// Conversation-level identifier (session ID).
        conversation_id: String,
        /// Turn-level identifier within the conversation.
        turn_id: String,
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
    /// Revoke a previously granted session-wide permission allow pattern (#177).
    RevokePattern {
        /// Action pattern to revoke.
        action: String,
        /// Target glob pattern to revoke.
        target_pattern: String,
    },
    /// Graceful shutdown.
    Shutdown,
    /// Liveness probe. The server must reply with [`IpcResponse::Pong`]
    /// promptly; a hung tool that fails to answer within the probe timeout
    /// is considered unhealthy and restarted by the host (#238).
    Ping,
    /// Poll the status of a deferred (background) task by id (#196).
    ///
    /// The server replies with [`IpcResponse::DeferredStatus`].
    PollDeferred {
        /// The `task_id` returned by a prior [`IpcResponse::DeferredAccepted`].
        task_id: String,
    },
    /// Cancel a deferred (background) task by id (#196).
    ///
    /// The server replies with [`IpcResponse::Ack`].
    CancelDeferred {
        /// The `task_id` of the background task to cancel.
        task_id: String,
    },
}

/// IPC response — host → core
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IpcResponse {
    /// Handshake acknowledgment with negotiated version.
    HandshakeAck {
        /// Agreed protocol version.
        version: u32,
    },
    /// Acknowledgment (for `ApprovePermission`, `AllowPattern`, etc.).
    Ack,
    /// List of tool specs.
    Tools {
        /// The structured tool specs.
        tools: Vec<ToolSpec>,
    },
    /// Host/RAG metadata profiles (#137).
    RagProfiles {
        /// Per-tool RAG profiles matching `Tools`.
        profiles: Vec<ToolRagProfile>,
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
    /// Acknowledgment of a deferred (background) tool call (#196).
    ///
    /// Returned instead of [`IpcResponse::CallResult`] when a
    /// background-capable tool accepts a deferred request. The actual
    /// result is delivered later out-of-band; the host tracks the task
    /// by `task_id`.
    DeferredAccepted {
        /// Unique identifier for the queued background task.
        task_id: String,
    },
    /// Error response.
    Error {
        /// Error description.
        message: String,
    },
    /// Reply to a [`IpcRequest::Ping`] liveness probe (#238).
    Pong,
    /// Status of a deferred (background) task (#196).
    ///
    /// Returned in response to [`IpcRequest::PollDeferred`].
    DeferredStatus {
        /// The polled task id.
        task_id: String,
        /// Current status of the task.
        status: DeferredStatus,
    },
}

/// Status of a deferred (background) tool task (#196).
///
/// Reported by [`IpcResponse::DeferredStatus`] in response to a
/// [`IpcRequest::PollDeferred`] probe. The host/runtime polls a
/// background-capable tool until the task reaches a terminal state
/// ([`DeferredStatus::Completed`], [`DeferredStatus::Failed`], or
/// [`DeferredStatus::Cancelled`]) and then surfaces the result as a
/// background-completed event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferredStatus {
    /// The task is still running.
    Pending,
    /// The task finished successfully with `result`.
    Completed {
        /// The tool's output string.
        result: String,
    },
    /// The task failed with `error`.
    Failed {
        /// Human-readable failure description.
        error: String,
    },
    /// The task was cancelled before completing.
    Cancelled,
    /// No task with the polled id is known to the tool.
    Unknown,
}

/// Call context identifying the conversation and turn for which
/// a tool is being invoked. Carried from the runtime to every tool
/// via [`crate::ToolProvider::set_call_context`] so tool-side logic
/// can implement per-turn scoping (e.g. undo checkpoints, ephemeral
/// scratch directories, per-turn access counters).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CallContext {
    /// Conversation-level identifier (session ID).
    pub conversation_id: String,
    /// Turn-level identifier within the conversation.
    pub turn_id: String,
}

/// Accessor for tool configuration.
///
/// Cheap to clone (shares the underlying `Arc<RwLock<…>>`), so it can be
/// handed to multiple tasks that need concurrent read/write access to the
/// same config value.
#[derive(Clone)]
pub struct ToolConfigAccessor {
    config: std::sync::Arc<tokio::sync::RwLock<serde_json::Value>>,
}

impl ToolConfigAccessor {
    /// Wraps an initial JSON config value for shared read/write access.
    pub fn new(initial_config: serde_json::Value) -> Self {
        Self {
            config: std::sync::Arc::new(tokio::sync::RwLock::new(initial_config)),
        }
    }

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

    /// Overwrite the stored config with a new serializable value.
    #[expect(
        clippy::future_not_send,
        reason = "IPC transport uses single-threaded runtime on some targets"
    )]
    pub async fn set<T: serde::Serialize>(&self, config: &T) -> Result<(), ToolError> {
        let val = serde_json::to_value(config).map_err(|e| ToolError::InvalidArguments {
            message: format!("Failed to serialize config: {e}"),
        })?;
        let mut guard = self.config.write().await;
        *guard = val;
        drop(guard);
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
        return Err(ToolError::ipc_transport(format!(
            "Request size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
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
        return Err(ToolError::ipc_transport(format!(
            "Response size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
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
    use crate::sandbox::SandboxConfigData;
    use crate::tool_types::{ToolName, ToolSpec};

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
    async fn ipc_request_handshake_v3_roundtrip() {
        let sandbox = SandboxConfigData {
            enabled: true,
            allowed_directories: vec![],
            writable_directories: vec![],
            db_socket: None,
            db_auth_token: None,
            ..SandboxConfigData::default()
        };
        let req = IpcRequest::Handshake {
            version: IPC_PROTOCOL_VERSION,
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
    async fn ipc_request_call_tool_roundtrip() {
        let req = IpcRequest::CallTool {
            name: "read".into(),
            arguments: r#"{"path":"/tmp/test.txt"}"#.into(),
            deferred: false,
            context: None,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_call_tool_deferred_roundtrip() {
        let req = IpcRequest::CallTool {
            name: "timer.set".into(),
            arguments: r#"{"seconds":5}"#.into(),
            deferred: true,
            context: None,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_call_tool_defaults_to_sync() {
        // A v1-shaped payload without the `deferred` field must
        // deserialize to a synchronous call for backward compatibility.
        let json = r#"{"CallTool":{"name":"read","arguments":"{}"}}"#;
        let req: IpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            IpcRequest::CallTool {
                name: "read".into(),
                arguments: "{}".into(),
                deferred: false,
                context: None,
            }
        );
    }

    #[tokio::test]
    async fn ipc_response_deferred_accepted_roundtrip() {
        let resp = IpcResponse::DeferredAccepted {
            task_id: "task-123".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_request_poll_deferred_roundtrip() {
        let req = IpcRequest::PollDeferred {
            task_id: "task-456".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_request_cancel_deferred_roundtrip() {
        let req = IpcRequest::CancelDeferred {
            task_id: "task-789".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn ipc_response_deferred_status_roundtrip() {
        use super::DeferredStatus;
        let resp = IpcResponse::DeferredStatus {
            task_id: "task-abc".into(),
            status: DeferredStatus::Completed {
                result: "done".into(),
            },
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn ipc_request_shutdown_roundtrip() {
        let req = IpcRequest::Shutdown;
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
    async fn ipc_response_pong_roundtrip() {
        let resp = IpcResponse::Pong;
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
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
        assert!(
            matches!(got, IpcResponse::CallResult { .. }),
            "Expected CallResult, got {got:?}"
        );
        let IpcResponse::CallResult { result } = got else {
            return;
        };
        assert_eq!(result.unwrap(), big_content);
    }

    // ── ToolConfigAccessor tests ────────────────────────────────────

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct SampleConfig {
        threshold: u32,
        label: String,
    }

    #[tokio::test]
    async fn config_accessor_get_success() {
        let accessor = ToolConfigAccessor::new(serde_json::json!({
            "threshold": 42,
            "label": "hello"
        }));
        let cfg: SampleConfig = accessor.get().await.unwrap();
        assert_eq!(cfg.threshold, 42);
        assert_eq!(cfg.label, "hello");
    }

    #[tokio::test]
    async fn config_accessor_get_type_mismatch() {
        let accessor = ToolConfigAccessor::new(serde_json::json!({
            "threshold": "not_a_number",
            "label": 123
        }));
        let result: Result<SampleConfig, _> = accessor.get().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("does not match expected type"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn config_accessor_set_success() {
        let accessor = ToolConfigAccessor::new(serde_json::json!({}));
        let new_cfg = SampleConfig {
            threshold: 99,
            label: "updated".into(),
        };
        accessor.set(&new_cfg).await.unwrap();
        let got: SampleConfig = accessor.get().await.unwrap();
        assert_eq!(got, new_cfg);
    }

    #[tokio::test]
    async fn config_accessor_set_overwrites_previous_value() {
        let accessor = ToolConfigAccessor::new(serde_json::json!({"threshold": 1, "label": "a"}));
        accessor
            .set(&SampleConfig {
                threshold: 2,
                label: "b".into(),
            })
            .await
            .unwrap();
        accessor
            .set(&SampleConfig {
                threshold: 3,
                label: "c".into(),
            })
            .await
            .unwrap();
        let got: SampleConfig = accessor.get().await.unwrap();
        assert_eq!(got.threshold, 3);
        assert_eq!(got.label, "c");
    }

    #[tokio::test]
    async fn config_accessor_concurrent_read_write() {
        let accessor =
            ToolConfigAccessor::new(serde_json::json!({"threshold": 0, "label": "init"}));
        let writer = accessor.clone();
        let writer_handle = tokio::spawn(async move {
            for i in 0..50u32 {
                let cfg = SampleConfig {
                    threshold: i,
                    label: format!("iter-{i}"),
                };
                writer.set(&cfg).await.unwrap();
            }
        });

        let reader = accessor.clone();
        let reader_handle = tokio::spawn(async move {
            for _ in 0..50 {
                // Reads must always produce a valid SampleConfig (never a
                // torn or partially-written value).
                let cfg: SampleConfig = reader.get().await.unwrap();
                assert!(cfg.label.starts_with("iter-") || cfg.label == "init");
            }
        });

        writer_handle.await.unwrap();
        reader_handle.await.unwrap();

        // Final value must be the last written.
        let final_cfg: SampleConfig = accessor.get().await.unwrap();
        assert_eq!(final_cfg.threshold, 49);
        assert_eq!(final_cfg.label, "iter-49");
    }
}
