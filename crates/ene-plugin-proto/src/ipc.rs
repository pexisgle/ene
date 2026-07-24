//! Plugin IPC wire protocol (protocol version 3).
//!
//! Extends the tool IPC (v2) with streaming LLM messages and a richer
//! handshake that carries [`PluginCapabilities`]. The framing is identical:
//! 4-byte little-endian length prefix followed by a JSON payload.

use crate::capabilities::PluginCapabilities;
use crate::error::PluginError;
use crate::sandbox::SandboxConfigData;
use crate::tool_error::ToolError;
use crate::tool_ipc::DeferredStatus;
use crate::tool_types::ToolSpec;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Maximum allowed IPC message size in bytes (64 MB).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Plugin IPC protocol version.
///
/// v3 extends the tool IPC v2 with:
/// - `Handshake` gains `plugin_config` (replaces `tool_config` for plugins)
/// - `HandshakeAck` gains `capabilities: PluginCapabilities`
/// - Streaming LLM messages (`CreateChatStream`, `StreamChunk`, `StreamEnd`,
///   `StreamError`)
/// - `ChatCompletion` / `EmbedBatch` for non-streaming provider calls
pub const PLUGIN_IPC_PROTOCOL_VERSION: u32 = 3;

/// Plugin IPC request — host → plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginIpcRequest {
    /// Handshake to negotiate protocol version, exchange sandbox config,
    /// and push plugin-specific configuration.
    Handshake {
        /// Client's supported protocol version.
        version: u32,
        /// Sandbox configuration to apply.
        #[serde(default)]
        sandbox: SandboxConfigData,
        /// Plugin-specific configuration JSON.
        plugin_config: Option<serde_json::Value>,
    },
    /// Graceful shutdown.
    Shutdown,
    /// Liveness probe. The plugin must reply with [`PluginIpcResponse::Pong`].
    Ping,
    /// Request the plugin's config JSON Schema.
    GetConfigSchema,
    /// List all available tool specs.
    ListTools,
    /// Execute a tool by name with JSON arguments.
    CallTool {
        /// Tool name to call.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
        /// When `true`, request deferred (background) execution.
        #[serde(default)]
        deferred: bool,
    },
    /// Set the call context (conversation + turn identifiers).
    SetCallContext {
        /// Conversation-level identifier.
        conversation_id: String,
        /// Turn-level identifier within the conversation.
        turn_id: String,
    },
    /// Poll the status of a deferred (background) task by id.
    PollDeferred {
        /// The `task_id` returned by a prior [`PluginIpcResponse::DeferredAccepted`].
        task_id: String,
    },
    /// Cancel a deferred (background) task by id.
    CancelDeferred {
        /// The `task_id` of the background task to cancel.
        task_id: String,
    },
    /// Approve a pending permission request.
    ApprovePermission {
        /// ID of the request to approve.
        request_id: String,
    },
    /// Register a session-wide permission allow pattern.
    AllowPattern {
        /// Action pattern (e.g. `"filesystem_write"`).
        action: String,
        /// Target glob pattern.
        target_pattern: String,
    },
    /// Revoke a previously granted session-wide permission allow pattern.
    RevokePattern {
        /// Action pattern to revoke.
        action: String,
        /// Target glob pattern to revoke.
        target_pattern: String,
    },
    /// Create a streaming chat completion.
    ///
    /// The plugin responds with N × [`PluginIpcResponse::StreamChunk`]
    /// followed by a terminal [`PluginIpcResponse::StreamEnd`] or
    /// [`PluginIpcResponse::StreamError`], all carrying the same `request_id`.
    CreateChatStream {
        /// Unique request identifier for correlating stream responses.
        request_id: String,
        /// Provider kind (e.g. `"anthropic"`).
        provider_kind: String,
        /// Provider-specific configuration JSON (API keys, base URL, etc.).
        provider_config: serde_json::Value,
        /// Model identifier.
        model: String,
        /// Maximum tokens to generate.
        max_tokens: Option<u32>,
        /// Chat messages in provider-agnostic JSON format.
        messages: Vec<serde_json::Value>,
        /// Tool definitions available for the model to call.
        #[serde(default)]
        tools: Vec<serde_json::Value>,
    },
    /// Non-streaming chat completion.
    ///
    /// The plugin responds with [`PluginIpcResponse::ChatCompletionResult`].
    ChatCompletion {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"anthropic"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Model identifier.
        model: String,
        /// Maximum tokens to generate.
        max_tokens: Option<u32>,
        /// Chat messages in provider-agnostic JSON format.
        messages: Vec<serde_json::Value>,
        /// Optional JSON Schema for structured output.
        json_schema: Option<serde_json::Value>,
    },
    /// Batch embedding request.
    ///
    /// The plugin responds with [`PluginIpcResponse::EmbedBatchResult`].
    EmbedBatch {
        /// Unique request identifier.
        request_id: String,
        /// Provider kind (e.g. `"openai_compatible"`).
        provider_kind: String,
        /// Provider-specific configuration JSON.
        provider_config: serde_json::Value,
        /// Embedding model identifier.
        model: String,
        /// Desired embedding dimensions (provider may ignore).
        dimensions: Option<u32>,
        /// Text items to embed.
        items: Vec<String>,
    },
}

/// Plugin IPC response — plugin → host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PluginIpcResponse {
    /// Handshake acknowledgment with negotiated version and capabilities.
    HandshakeAck {
        /// Agreed protocol version.
        version: u32,
        /// Capabilities advertised by this plugin.
        capabilities: PluginCapabilities,
    },
    /// Acknowledgment (for `ApprovePermission`, `AllowPattern`, etc.).
    Ack,
    /// Reply to a [`PluginIpcRequest::Ping`] liveness probe.
    Pong,
    /// The plugin's config JSON Schema.
    ConfigSchema {
        /// The schema, or `None` if not provided.
        schema: Option<serde_json::Value>,
    },
    /// Error response.
    Error {
        /// Error description.
        message: String,
    },
    /// List of tool specs.
    Tools {
        /// The structured tool specs.
        tools: Vec<ToolSpec>,
    },
    /// Result of a tool call.
    CallResult {
        /// The result, or an error.
        result: Result<String, ToolError>,
    },
    /// Acknowledgment of a deferred (background) tool call.
    DeferredAccepted {
        /// Unique identifier for the queued background task.
        task_id: String,
    },
    /// Status of a deferred (background) task.
    DeferredStatus {
        /// The polled task id.
        task_id: String,
        /// Current status of the task.
        status: DeferredStatus,
    },
    /// A streaming chunk for an in-progress chat stream.
    ///
    /// Multiple chunks are sent per `request_id`, each carrying incremental
    /// text and/or tool-call deltas.
    StreamChunk {
        /// Request identifier correlating this chunk to the originating
        /// [`PluginIpcRequest::CreateChatStream`].
        request_id: String,
        /// Incremental text content (may be empty if only tool calls advance).
        #[serde(default)]
        text_delta: String,
        /// Incremental tool-call JSON (partial function-call arguments).
        #[serde(default)]
        tool_calls_delta: Vec<serde_json::Value>,
    },
    /// Terminal message indicating a stream completed successfully.
    StreamEnd {
        /// Request identifier for the completed stream.
        request_id: String,
    },
    /// Terminal message indicating a stream failed.
    StreamError {
        /// Request identifier for the failed stream.
        request_id: String,
        /// Human-readable error description.
        message: String,
    },
    /// Result of a non-streaming chat completion.
    ChatCompletionResult {
        /// Request identifier.
        request_id: String,
        /// The generated content.
        content: String,
    },
    /// Result of a batch embedding request.
    EmbedBatchResult {
        /// Request identifier.
        request_id: String,
        /// Embedding vectors, one per input item.
        embeddings: Vec<Vec<f32>>,
    },
}

/// Reads a [`PluginIpcRequest`] as 4-byte LE length-prefixed JSON.
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed.
pub async fn read_plugin_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<PluginIpcRequest>, PluginError> {
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
        return Err(PluginError::protocol(format!(
            "Request size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let req = serde_json::from_slice(&buf).map_err(|e| {
        PluginError::protocol(format!("Failed to deserialize PluginIpcRequest: {e}"))
    })?;
    Ok(Some(req))
}

/// Writes a [`PluginIpcRequest`] as 4-byte LE length-prefixed JSON.
///
/// # Errors
///
/// Returns [`PluginError::Protocol`] if the serialized message exceeds
/// [`MAX_MESSAGE_SIZE`], preventing oversized messages from being silently
/// sent to the peer.
pub async fn write_plugin_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    req: &PluginIpcRequest,
) -> Result<(), PluginError> {
    let json = serde_json::to_vec(req)
        .map_err(|e| PluginError::protocol(format!("Failed to serialize PluginIpcRequest: {e}")))?;
    if json.len() > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Request size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            json.len()
        )));
    }
    let len = json.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads a [`PluginIpcResponse`] as 4-byte LE length-prefixed JSON.
///
/// Returns `Ok(None)` on `UnexpectedEof`, indicating connection closed.
///
/// # Correlation invariant
///
/// Non-streaming responses (`Ack`, `Pong`, `ConfigSchema`, `CallResult`,
/// `DeferredAccepted`, `DeferredStatus`, `ChatCompletionResult`,
/// `EmbedBatchResult`, `Error`) carry **no `request_id`**, so the reader
/// cannot correlate a response to a specific request. Callers must therefore
/// ensure that **at most one non-streaming request is in-flight per
/// connection** at any time. Streaming responses (`StreamChunk`, `StreamEnd`,
/// `StreamError`) do carry a `request_id` and are not subject to this
/// constraint.
pub async fn read_plugin_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<PluginIpcResponse>, PluginError> {
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
        return Err(PluginError::protocol(format!(
            "Response size {len} exceeds maximum {MAX_MESSAGE_SIZE}"
        )));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    let resp = serde_json::from_slice(&buf).map_err(|e| {
        PluginError::protocol(format!("Failed to deserialize PluginIpcResponse: {e}"))
    })?;
    Ok(Some(resp))
}

/// Writes a [`PluginIpcResponse`] as 4-byte LE length-prefixed JSON.
///
/// # Errors
///
/// Returns [`PluginError::Protocol`] if the serialized message exceeds
/// [`MAX_MESSAGE_SIZE`], preventing oversized messages from being silently
/// sent to the peer.
pub async fn write_plugin_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    resp: &PluginIpcResponse,
) -> Result<(), PluginError> {
    let json = serde_json::to_vec(resp).map_err(|e| {
        PluginError::protocol(format!("Failed to serialize PluginIpcResponse: {e}"))
    })?;
    if json.len() > MAX_MESSAGE_SIZE {
        return Err(PluginError::protocol(format!(
            "Response size {} exceeds maximum {MAX_MESSAGE_SIZE}",
            json.len()
        )));
    }
    let len = json.len() as u32;
    writer.write_all(&len.to_le_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::LlmProviderSpec;

    async fn send_recv_request(req: &PluginIpcRequest) -> PluginIpcRequest {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        write_plugin_request(&mut a, req).await.unwrap();
        drop(a);
        read_plugin_request(&mut b).await.unwrap().unwrap()
    }

    async fn send_recv_response(resp: &PluginIpcResponse) -> PluginIpcResponse {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        write_plugin_response(&mut a, resp).await.unwrap();
        drop(a);
        read_plugin_response(&mut b).await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn request_handshake_roundtrip() {
        let req = PluginIpcRequest::Handshake {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            sandbox: SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"api_key": "sk-test"})),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_shutdown_roundtrip() {
        let req = PluginIpcRequest::Shutdown;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_ping_roundtrip() {
        let req = PluginIpcRequest::Ping;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_get_config_schema_roundtrip() {
        let req = PluginIpcRequest::GetConfigSchema;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_list_tools_roundtrip() {
        let req = PluginIpcRequest::ListTools;
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_call_tool_roundtrip() {
        let req = PluginIpcRequest::CallTool {
            name: "filesystem.read".into(),
            arguments: r#"{"path":"/tmp/test.txt"}"#.into(),
            deferred: false,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_call_tool_deferred_roundtrip() {
        let req = PluginIpcRequest::CallTool {
            name: "timer.set".into(),
            arguments: r#"{"seconds":5}"#.into(),
            deferred: true,
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_set_call_context_roundtrip() {
        let req = PluginIpcRequest::SetCallContext {
            conversation_id: "conv-1".into(),
            turn_id: "turn-42".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_poll_deferred_roundtrip() {
        let req = PluginIpcRequest::PollDeferred {
            task_id: "task-123".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_cancel_deferred_roundtrip() {
        let req = PluginIpcRequest::CancelDeferred {
            task_id: "task-456".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_approve_permission_roundtrip() {
        let req = PluginIpcRequest::ApprovePermission {
            request_id: "perm-1".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_allow_pattern_roundtrip() {
        let req = PluginIpcRequest::AllowPattern {
            action: "filesystem_write".into(),
            target_pattern: "/tmp/**".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_revoke_pattern_roundtrip() {
        let req = PluginIpcRequest::RevokePattern {
            action: "filesystem_write".into(),
            target_pattern: "/tmp/**".into(),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_create_chat_stream_roundtrip() {
        let req = PluginIpcRequest::CreateChatStream {
            request_id: "req-uuid-1".into(),
            provider_kind: "anthropic".into(),
            provider_config: serde_json::json!({"api_key": "sk-ant-test"}),
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: Some(4096),
            messages: vec![serde_json::json!({"role": "user", "content": "Hello"})],
            tools: vec![serde_json::json!({"name": "read", "description": "Read a file"})],
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_chat_completion_roundtrip() {
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-uuid-2".into(),
            provider_kind: "anthropic".into(),
            provider_config: serde_json::json!({}),
            model: "claude-sonnet-4-20250514".into(),
            max_tokens: None,
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            json_schema: Some(serde_json::json!({"type": "object"})),
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn request_embed_batch_roundtrip() {
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-uuid-3".into(),
            provider_kind: "openai_compatible".into(),
            provider_config: serde_json::json!({"base_url": "http://localhost:11434"}),
            model: "text-embedding-3-small".into(),
            dimensions: Some(1536),
            items: vec!["hello world".into(), "foo bar".into()],
        };
        let got = send_recv_request(&req).await;
        assert_eq!(got, req);
    }

    #[tokio::test]
    async fn response_handshake_ack_roundtrip() {
        let resp = PluginIpcResponse::HandshakeAck {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            capabilities: PluginCapabilities {
                tools: vec![],
                llm_providers: vec![LlmProviderSpec {
                    kind: "anthropic".into(),
                    supported_models: vec!["claude-sonnet-4-20250514".into()],
                    supports_streaming: true,
                    supports_vision: true,
                }],
                tts_providers: vec![],
                stt_providers: vec![],
            },
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_ack_roundtrip() {
        let resp = PluginIpcResponse::Ack;
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_pong_roundtrip() {
        let resp = PluginIpcResponse::Pong;
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_config_schema_roundtrip() {
        let resp = PluginIpcResponse::ConfigSchema {
            schema: Some(serde_json::json!({"type": "object", "properties": {}})),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_error_roundtrip() {
        let resp = PluginIpcResponse::Error {
            message: "something went wrong".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_tools_roundtrip() {
        let resp = PluginIpcResponse::Tools {
            tools: vec![ToolSpec::new(
                crate::ToolName::new("test"),
                "desc",
                serde_json::json!({}),
            )],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_call_result_ok_roundtrip() {
        let resp = PluginIpcResponse::CallResult {
            result: Ok("success".into()),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_call_result_err_roundtrip() {
        let resp = PluginIpcResponse::CallResult {
            result: Err(ToolError::NotFound {
                tool_name: "foo".into(),
            }),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_accepted_roundtrip() {
        let resp = PluginIpcResponse::DeferredAccepted {
            task_id: "task-789".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_deferred_status_roundtrip() {
        let resp = PluginIpcResponse::DeferredStatus {
            task_id: "task-abc".into(),
            status: DeferredStatus::Completed {
                result: "done".into(),
            },
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_chunk_roundtrip() {
        let resp = PluginIpcResponse::StreamChunk {
            request_id: "req-uuid-1".into(),
            text_delta: "Hello, ".into(),
            tool_calls_delta: vec![],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_chunk_with_tool_calls_roundtrip() {
        let resp = PluginIpcResponse::StreamChunk {
            request_id: "req-uuid-1".into(),
            text_delta: String::new(),
            tool_calls_delta: vec![
                serde_json::json!({"id": "tc_1", "name": "read", "arguments": "{\"pa"}),
            ],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_end_roundtrip() {
        let resp = PluginIpcResponse::StreamEnd {
            request_id: "req-uuid-1".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_stream_error_roundtrip() {
        let resp = PluginIpcResponse::StreamError {
            request_id: "req-uuid-1".into(),
            message: "rate limit exceeded".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_chat_completion_result_roundtrip() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "req-uuid-2".into(),
            content: "The answer is 42.".into(),
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn response_embed_batch_result_roundtrip() {
        let resp = PluginIpcResponse::EmbedBatchResult {
            request_id: "req-uuid-3".into(),
            embeddings: vec![vec![0.1, 0.2, 0.3], vec![0.4, 0.5, 0.6]],
        };
        let got = send_recv_response(&resp).await;
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn read_request_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_plugin_request(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn read_response_eof_returns_none() {
        let mut buf: &[u8] = &[];
        let result = read_plugin_response(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zero_length_request_returns_none() {
        let mut buf: &[u8] = &[0u8; 4];
        let result = read_plugin_request(&mut buf).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn streaming_sequence_roundtrip() {
        let (mut writer, mut reader) = tokio::io::duplex(64 * 1024);

        let chunks = vec![
            PluginIpcResponse::StreamChunk {
                request_id: "stream-1".into(),
                text_delta: "Hello".into(),
                tool_calls_delta: vec![],
            },
            PluginIpcResponse::StreamChunk {
                request_id: "stream-1".into(),
                text_delta: ", world!".into(),
                tool_calls_delta: vec![],
            },
            PluginIpcResponse::StreamEnd {
                request_id: "stream-1".into(),
            },
        ];

        let expected = chunks.clone();
        let write_handle = tokio::spawn(async move {
            for chunk in &chunks {
                write_plugin_response(&mut writer, chunk).await.unwrap();
            }
        });

        let mut received = Vec::new();
        loop {
            let resp = read_plugin_response(&mut reader).await.unwrap().unwrap();
            let is_terminal = matches!(
                resp,
                PluginIpcResponse::StreamEnd { .. } | PluginIpcResponse::StreamError { .. }
            );
            received.push(resp);
            if is_terminal {
                break;
            }
        }

        write_handle.await.unwrap();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn large_payload_roundtrip() {
        let big_content = "x".repeat(100_000);
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "big-1".into(),
            content: big_content.clone(),
        };
        let (mut a, mut b) = tokio::io::duplex(256 * 1024);
        write_plugin_response(&mut a, &resp).await.unwrap();
        drop(a);
        let got = read_plugin_response(&mut b).await.unwrap().unwrap();
        assert_eq!(got, resp);
    }

    #[tokio::test]
    async fn call_tool_defaults_to_sync() {
        let json = r#"{"CallTool":{"name":"read","arguments":"{}"}}"#;
        let req: PluginIpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req,
            PluginIpcRequest::CallTool {
                name: "read".into(),
                arguments: "{}".into(),
                deferred: false,
            }
        );
    }

    #[tokio::test]
    async fn read_request_rejects_oversized_message() {
        let oversized_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut buf = oversized_len.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        let mut cursor: &[u8] = &buf;
        let result = read_plugin_request(&mut cursor).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn read_response_rejects_oversized_message() {
        let oversized_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut buf = oversized_len.to_le_bytes().to_vec();
        buf.extend_from_slice(&[0u8; 16]);
        let mut cursor: &[u8] = &buf;
        let result = read_plugin_response(&mut cursor).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
    }

    #[tokio::test]
    async fn write_request_rejects_oversized_message() {
        let big_arguments = "x".repeat(MAX_MESSAGE_SIZE + 1);
        let req = PluginIpcRequest::CallTool {
            name: "test".into(),
            arguments: big_arguments,
            deferred: false,
        };
        let mut buf = Vec::new();
        let result = write_plugin_request(&mut buf, &req).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }

    #[tokio::test]
    async fn write_response_rejects_oversized_message() {
        let resp = PluginIpcResponse::ChatCompletionResult {
            request_id: "big-1".into(),
            content: "x".repeat(MAX_MESSAGE_SIZE + 1),
        };
        let mut buf = Vec::new();
        let result = write_plugin_response(&mut buf, &resp).await;
        let err = result.unwrap_err();
        assert!(matches!(err, PluginError::Protocol(_)));
        assert!(err.to_string().contains("exceeds maximum"));
        assert!(buf.is_empty(), "nothing must be written on rejection");
    }
}
