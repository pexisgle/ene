//! Plugin IPC server: accept loop, request dispatch, and streaming handler.
//!
//! [`run_plugin_server`] is the entry point for plugin binaries. It binds an
//! IPC listener, accepts connections, and dispatches
//! [`PluginIpcRequest`](ene_plugin_proto::PluginIpcRequest) messages to the
//! appropriate trait implementation via [`PluginDispatch`].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ene_plugin_proto::{DeferredOutcome, VersionRange};
use ene_plugin_proto::{
    IpcListener, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginError,
    PluginIpcRequest, PluginIpcResponse, cleanup_path, read_plugin_request, write_plugin_response,
};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::plugin::{EmbedPlugin, LlmPlugin, SttPlugin, ToolPlugin, TtsPlugin};

/// Dispatch table holding up to five focused trait implementations.
///
/// A plugin struct implements any subset of [`ToolPlugin`], [`LlmPlugin`],
/// [`EmbedPlugin`], [`TtsPlugin`], and [`SttPlugin`]; the server routes
/// incoming IPC requests to the corresponding trait object.
pub struct PluginDispatch {
    /// Optional tool plugin implementation.
    pub tool: Option<Arc<dyn ToolPlugin>>,
    /// Optional LLM plugin implementation.
    pub llm: Option<Arc<dyn LlmPlugin>>,
    /// Optional embedding plugin implementation.
    pub embed: Option<Arc<dyn EmbedPlugin>>,
    /// Optional TTS plugin implementation.
    pub tts: Option<Arc<dyn TtsPlugin>>,
    /// Optional STT plugin implementation.
    pub stt: Option<Arc<dyn SttPlugin>>,
}

impl PluginDispatch {
    /// Creates a dispatch table with the given trait implementations.
    pub fn new(
        tool: Option<Arc<dyn ToolPlugin>>,
        llm: Option<Arc<dyn LlmPlugin>>,
        embed: Option<Arc<dyn EmbedPlugin>>,
        tts: Option<Arc<dyn TtsPlugin>>,
        stt: Option<Arc<dyn SttPlugin>>,
    ) -> Self {
        Self {
            tool,
            llm,
            embed,
            tts,
            stt,
        }
    }
}

/// Starts a plugin as an IPC server.
///
/// Reads the socket path from the `ENE_PLUGIN_SOCKET` environment variable
/// (falling back to `/tmp/ene-plugin.sock` on Unix or `\\.\pipe\ene-plugin`
/// on Windows) and listens for requests over IPC. Shuts down upon receiving
/// a `Shutdown` request, `SIGINT`, or `SIGTERM`.
///
/// # Usage
///
/// ```rust,no_run
/// # use ene_plugin::{PluginDispatch, ToolPlugin, PluginStreamChunk};
/// # struct MyTool;
/// # #[async_trait::async_trait]
/// # impl ToolPlugin for MyTool {
/// #     fn tool_capabilities(&self) -> ene_plugin::ToolPluginCapabilities {
/// #         ene_plugin::ToolPluginCapabilities { tool_count: 0 }
/// #     }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), ene_plugin::PluginError> {
///     ene_plugin::run_plugin_server(PluginDispatch::new(
///         Some(std::sync::Arc::new(MyTool)),
///         None,
///         None,
///         None,
///         None,
///     )).await
/// }
/// ```
pub async fn run_plugin_server(dispatch: PluginDispatch) -> Result<(), PluginError> {
    let socket_path = std::env::var("ENE_PLUGIN_SOCKET").unwrap_or_else(|_| {
        #[cfg(unix)]
        {
            "/tmp/ene-plugin.sock".to_string()
        }
        #[cfg(windows)]
        {
            r"\\.\pipe\ene-plugin".to_string()
        }
    });
    let socket_path = PathBuf::from(&socket_path);

    cleanup_path(&socket_path);

    let mut listener = IpcListener::bind(&socket_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, perms)?;
    }

    tracing::info!(
        component = "PluginServer",
        socket = %socket_path.display(),
        "Listening"
    );

    let dispatch: Arc<PluginDispatch> = Arc::new(dispatch);
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let tasks: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        #[cfg(unix)]
        let sigterm_fut = sigterm.recv();
        #[cfg(not(unix))]
        let sigterm_fut = std::future::pending::<()>();

        tokio::select! {
            result = listener.accept() => {
                let stream = result?;
                let dispatch = Arc::clone(&dispatch);
                let shutdown = Arc::clone(&shutdown);
                let tasks = Arc::clone(&tasks);
                let handle = tokio::spawn(async move {
                    handle_connection(dispatch, stream, shutdown).await;
                });
                tasks.lock().await.push(handle);
            }
            () = shutdown.notified() => {
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "PluginServer", "Received SIGINT, shutting down");
                break;
            }
            _ = sigterm_fut => {
                tracing::info!(component = "PluginServer", "Received SIGTERM, shutting down");
                break;
            }
        }
    }

    // Join all spawned connection tasks before cleanup. This prevents
    // orphaned tasks from writing to a closed socket or holding references
    // to the dispatch Arc after the server has exited.
    {
        let mut guard = tasks.lock().await;
        let handles: Vec<_> = guard.drain(..).collect();
        drop(guard);
        for handle in handles {
            let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        }
    }

    cleanup_path(&socket_path);
    tracing::info!(component = "PluginServer", "Shut down");
    Ok(())
}

/// Handles a single IPC connection: reads requests in a loop, dispatches
/// them, and writes responses back. Exits on EOF, `Shutdown`, or I/O error.
async fn handle_connection(
    dispatch: Arc<PluginDispatch>,
    mut stream: IpcStream,
    shutdown: Arc<tokio::sync::Notify>,
) {
    loop {
        let req = match read_plugin_request(&mut stream).await {
            Ok(None) => break,
            Ok(Some(req)) => req,
            Err(e) => {
                tracing::error!(component = "PluginServer", error = %e, "IPC read error");
                let _ = write_plugin_response(
                    &mut stream,
                    &PluginIpcResponse::Error {
                        request_id: String::new(),
                        message: e.to_string(),
                    },
                )
                .await;
                break;
            }
        };

        let is_shutdown = matches!(req, PluginIpcRequest::Shutdown);

        if let Err(e) = handle_request(&dispatch, &req, &mut stream).await {
            tracing::error!(
                component = "PluginServer",
                error = %e,
                "Failed to handle request"
            );
            break;
        }

        // Drain any pending deferred task completions and push them to the host.
        if let Some(tool) = &dispatch.tool {
            for (task_id, result) in tool.drain_deferred_completions() {
                let resp = PluginIpcResponse::DeferredCompleted { task_id, result };
                if write_plugin_response(&mut stream, &resp).await.is_err() {
                    return;
                }
            }
        }

        if is_shutdown {
            shutdown.notify_one();
            break;
        }
    }
}

/// Dispatches a single request, writing one or more responses to `writer`.
///
/// Non-streaming requests produce exactly one response via [`dispatch`].
/// `CreateChatStream` produces N × `StreamChunk` followed by a terminal
/// `StreamEnd` or `StreamError` via [`handle_chat_stream`].
async fn handle_request<W: AsyncWriteExt + Unpin>(
    dispatch: &PluginDispatch,
    req: &PluginIpcRequest,
    writer: &mut W,
) -> Result<(), PluginError> {
    if matches!(req, PluginIpcRequest::CreateChatStream { .. }) {
        handle_chat_stream(dispatch, req, writer).await
    } else {
        let resp = dispatch_request(dispatch, req).await;
        write_plugin_response(writer, &resp).await
    }
}

/// Dispatches a non-streaming request and returns the single response.
#[expect(
    clippy::manual_let_else,
    reason = "match-based return is clearer for multi-branch dispatch with early returns"
)]
async fn dispatch_request(dispatch: &PluginDispatch, req: &PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            sandbox,
            plugin_config,
        } => {
            let our_range = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            };
            if host_range.max < our_range.min || host_range.min > our_range.max {
                tracing::error!(
                    component = "PluginServer",
                    host_min = host_range.min,
                    host_max = host_range.max,
                    our_min = our_range.min,
                    our_max = our_range.max,
                    "Handshake version range mismatch"
                );
                return PluginIpcResponse::Error {
                    request_id: String::new(),
                    message: format!(
                        "protocol version mismatch: host supports {}-{}, \
                         plugin supports {}-{}",
                        host_range.min, host_range.max, our_range.min, our_range.max
                    ),
                };
            }
            let negotiated = host_range.min.max(our_range.min);
            if let Some(tool) = &dispatch.tool {
                tool.set_sandbox(sandbox);
                if let Some(config) = plugin_config {
                    tool.set_config(config);
                }
            }
            PluginIpcResponse::HandshakeAck {
                version: negotiated,
                capabilities: collect_capabilities(dispatch),
            }
        }
        PluginIpcRequest::Ping => PluginIpcResponse::Pong,
        PluginIpcRequest::GetConfigSchema { request_id } => PluginIpcResponse::ConfigSchema {
            request_id: request_id.clone(),
            schema: dispatch.tool.as_ref().and_then(|t| t.config_schema()),
        },
        PluginIpcRequest::ListTools { request_id } => PluginIpcResponse::Tools {
            request_id: request_id.clone(),
            tools: dispatch
                .tool
                .as_ref()
                .map_or(Vec::new(), |t| t.list_tool_specs()),
        },
        PluginIpcRequest::CallTool {
            request_id,
            name,
            arguments,
            deferred,
            context,
        } => {
            let ctx_ref = context.as_ref();
            let Some(tool) = &dispatch.tool else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no tool plugin registered".to_string(),
                };
            };
            if *deferred {
                match tool.call_tool_deferred(name, arguments, ctx_ref).await {
                    Ok(DeferredOutcome::Sync(result)) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Ok(result),
                    },
                    Ok(DeferredOutcome::Deferred { task_id }) => {
                        PluginIpcResponse::DeferredAccepted {
                            request_id: request_id.clone(),
                            task_id,
                        }
                    }
                    Err(e) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Err(e),
                    },
                }
            } else {
                let result = tool.call_tool(name, arguments, ctx_ref).await;
                PluginIpcResponse::CallResult {
                    request_id: request_id.clone(),
                    result,
                }
            }
        }
        PluginIpcRequest::ChatCompletion {
            request_id,
            provider_kind,
            provider_config,
            model,
            max_tokens,
            messages,
            json_schema,
        } => {
            let llm = match &dispatch.llm {
                Some(l) => l,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no LLM plugin registered".to_string(),
                    };
                }
            };
            match llm
                .chat_completion(
                    provider_kind,
                    provider_config.clone(),
                    model.clone(),
                    *max_tokens,
                    messages.clone(),
                    json_schema.clone(),
                )
                .await
            {
                Ok(content) => PluginIpcResponse::ChatCompletionResult {
                    request_id: request_id.clone(),
                    content,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::EmbedBatch {
            request_id,
            provider_kind,
            provider_config,
            model,
            dimensions,
            items,
        } => {
            let embed = match &dispatch.embed {
                Some(e) => e,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no embed plugin registered".to_string(),
                    };
                }
            };
            match embed
                .embed_batch(
                    provider_kind,
                    provider_config.clone(),
                    model.clone(),
                    *dimensions,
                    items.clone(),
                )
                .await
            {
                Ok(embeddings) => PluginIpcResponse::EmbedBatchResult {
                    request_id: request_id.clone(),
                    embeddings,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::PollDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.poll_deferred(task_id) {
                Ok(status) => PluginIpcResponse::DeferredStatus {
                    request_id: request_id.clone(),
                    task_id: task_id.clone(),
                    status,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::SetCallContext {
            request_id,
            conversation_id,
            turn_id,
        } => {
            tracing::warn!(
                component = "PluginServer",
                "SetCallContext is deprecated; per-call context is now passed directly \
                 in CallTool requests (conversation_id={conversation_id}, turn_id={turn_id})"
            );
            PluginIpcResponse::Ack {
                request_id: request_id.clone(),
            }
        }
        PluginIpcRequest::ApprovePermission {
            request_id,
            permission_request_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.approve_permission(permission_request_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::AllowPattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.allow_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::RevokePattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.revoke_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::CancelStream { request_id, .. } => {
            // CancelStream is handled by the connection handler via a cancel map,
            // not dispatched to the plugin trait. Return an Ack for protocol compliance.
            PluginIpcResponse::Ack {
                request_id: request_id.clone(),
            }
        }
        PluginIpcRequest::CancelDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin registered".to_string(),
                    };
                }
            };
            match tool.cancel_deferred(task_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::SynthesizeSpeech {
            request_id,
            provider_kind,
            provider_config,
            text,
            voice,
            format,
        } => {
            let Some(tts) = &dispatch.tts else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no TTS plugin registered".to_string(),
                };
            };
            match tts
                .synthesize(
                    provider_kind,
                    provider_config.clone(),
                    text.clone(),
                    voice.clone(),
                    format.clone(),
                )
                .await
            {
                Ok(audio_data) => PluginIpcResponse::SpeechResult {
                    request_id: request_id.clone(),
                    audio_base64: base64_encode(&audio_data),
                    format: format.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::TranscribeAudio {
            request_id,
            provider_kind,
            provider_config,
            audio_base64,
            format,
        } => {
            let Some(stt) = &dispatch.stt else {
                return PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: "no STT plugin registered".to_string(),
                };
            };
            let audio_data = match base64_decode(audio_base64) {
                Ok(data) => data,
                Err(e) => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: format!("invalid base64 audio: {e}"),
                    };
                }
            };
            match stt
                .transcribe(
                    provider_kind,
                    provider_config.clone(),
                    audio_data,
                    format.clone(),
                )
                .await
            {
                Ok(text) => PluginIpcResponse::TranscriptionResult {
                    request_id: request_id.clone(),
                    text,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::Shutdown => PluginIpcResponse::Ack {
            request_id: String::new(),
        },
        // CreateChatStream is handled by handle_chat_stream, not here.
        PluginIpcRequest::CreateChatStream { .. } => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "CreateChatStream must be handled by the streaming path".to_string(),
        },
    }
}

/// Base64-encode bytes to a string.
fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Base64-decode a string to bytes.
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

/// Collects capabilities from all registered dispatch components.
fn collect_capabilities(dispatch: &PluginDispatch) -> PluginCapabilities {
    PluginCapabilities {
        tools: dispatch
            .tool
            .as_ref()
            .map_or(0, |t| t.tool_capabilities().tool_count),
        llm_providers: dispatch
            .llm
            .as_ref()
            .map_or(Vec::new(), |l| l.llm_capabilities()),
        tts_providers: dispatch
            .tts
            .as_ref()
            .map_or(Vec::new(), |t| t.tts_capabilities()),
        stt_providers: dispatch
            .stt
            .as_ref()
            .map_or(Vec::new(), |s| s.stt_capabilities()),
    }
}

/// Handles a `CreateChatStream` request by iterating the plugin's stream
/// and writing `StreamChunk` / `StreamEnd` / `StreamError` responses.
#[expect(
    clippy::manual_let_else,
    clippy::single_match_else,
    reason = "match-based return is clearer for early-exit patterns"
)]
async fn handle_chat_stream<W: AsyncWriteExt + Unpin>(
    dispatch: &PluginDispatch,
    req: &PluginIpcRequest,
    writer: &mut W,
) -> Result<(), PluginError> {
    let PluginIpcRequest::CreateChatStream {
        request_id,
        provider_kind,
        provider_config,
        model,
        max_tokens,
        messages,
        tools,
    } = req
    else {
        return Err(PluginError::protocol(
            "expected CreateChatStream request in streaming handler",
        ));
    };

    let llm = match &dispatch.llm {
        Some(l) => l,
        None => {
            let _ = write_plugin_response(
                writer,
                &PluginIpcResponse::StreamError {
                    request_id: request_id.clone(),
                    message: "no LLM plugin registered".to_string(),
                },
            )
            .await;
            return Ok(());
        }
    };

    match llm
        .create_chat_stream(
            provider_kind,
            provider_config.clone(),
            model.clone(),
            *max_tokens,
            messages.clone(),
            tools.clone(),
        )
        .await
    {
        Ok(mut stream) => {
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        let resp = PluginIpcResponse::StreamChunk {
                            request_id: request_id.clone(),
                            text_delta: chunk.text_delta.unwrap_or_default(),
                            tool_calls_delta: chunk.tool_calls_delta.unwrap_or_default(),
                        };
                        if write_plugin_response(writer, &resp).await.is_err() {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        let _ = write_plugin_response(
                            writer,
                            &PluginIpcResponse::StreamError {
                                request_id: request_id.clone(),
                                message: e.to_string(),
                            },
                        )
                        .await;
                        return Ok(());
                    }
                }
            }
            write_plugin_response(
                writer,
                &PluginIpcResponse::StreamEnd {
                    request_id: request_id.clone(),
                },
            )
            .await
        }
        Err(e) => {
            let _ = write_plugin_response(
                writer,
                &PluginIpcResponse::StreamError {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            )
            .await;
            Ok(())
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::panic,
    reason = "test assertions use panic for unexpected variants"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "test assertions index into known-length vectors"
)]
mod tests {
    use super::*;
    use crate::plugin::{PluginStreamChunk, ToolPluginCapabilities};
    use async_trait::async_trait;
    use ene_plugin_proto::ToolName;
    use ene_plugin_proto::{
        DeferredStatus, LlmProviderSpec, SttProviderSpec, ToolSpec, TtsProviderSpec, VersionRange,
    };

    /// A mock tool plugin for testing dispatch logic.
    struct MockToolPlugin;

    #[async_trait]
    impl ToolPlugin for MockToolPlugin {
        fn tool_capabilities(&self) -> ToolPluginCapabilities {
            ToolPluginCapabilities { tool_count: 1 }
        }

        fn list_tool_specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec::new(
                ToolName::new("test.echo"),
                "Echoes the input arguments.",
                serde_json::json!({}),
            )]
        }

        async fn call_tool(
            &self,
            name: &str,
            args: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, ene_plugin_proto::ToolError> {
            if name == "test.echo" {
                Ok(ene_plugin_proto::ToolResult::from_string(args.to_string()))
            } else {
                Err(ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                })
            }
        }

        fn config_schema(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({"type": "object"}))
        }
    }

    /// A mock LLM plugin for testing dispatch logic.
    struct MockLlmPlugin;

    #[async_trait]
    impl LlmPlugin for MockLlmPlugin {
        fn llm_capabilities(&self) -> Vec<LlmProviderSpec> {
            vec![LlmProviderSpec {
                kind: "mock".into(),
                supported_models: vec!["mock-model".into()],
                supports_streaming: true,
                supports_vision: false,
            }]
        }

        async fn create_chat_stream(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _max_tokens: Option<u32>,
            _messages: Vec<serde_json::Value>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<crate::plugin::PluginStream, PluginError> {
            let chunks = vec![
                Ok(PluginStreamChunk {
                    text_delta: Some("Hello".into()),
                    tool_calls_delta: None,
                }),
                Ok(PluginStreamChunk {
                    text_delta: Some(" world".into()),
                    tool_calls_delta: None,
                }),
            ];
            Ok(Box::pin(tokio_stream::iter(chunks)))
        }

        async fn chat_completion(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _max_tokens: Option<u32>,
            _messages: Vec<serde_json::Value>,
            _json_schema: Option<serde_json::Value>,
        ) -> Result<String, PluginError> {
            Ok("Mock completion response".into())
        }
    }

    /// A mock TTS plugin for testing dispatch logic.
    struct MockTtsPlugin;

    #[async_trait]
    impl TtsPlugin for MockTtsPlugin {
        fn tts_capabilities(&self) -> Vec<TtsProviderSpec> {
            vec![TtsProviderSpec {
                kind: "mock_tts".into(),
                voices: vec!["default".into()],
                formats: vec!["wav".into()],
            }]
        }

        async fn synthesize(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            text: String,
            _voice: String,
            _format: String,
        ) -> Result<Vec<u8>, PluginError> {
            Ok(text.into_bytes())
        }
    }

    /// A mock STT plugin for testing dispatch logic.
    struct MockSttPlugin;

    #[async_trait]
    impl SttPlugin for MockSttPlugin {
        fn stt_capabilities(&self) -> Vec<SttProviderSpec> {
            vec![SttProviderSpec {
                kind: "mock_stt".into(),
                models: vec!["mock-model".into()],
                formats: vec!["wav".into()],
            }]
        }

        async fn transcribe(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _audio_data: Vec<u8>,
            _format: String,
        ) -> Result<String, PluginError> {
            Ok("Mock transcription".into())
        }
    }

    /// A mock embed plugin for testing dispatch logic.
    struct MockEmbedPlugin;

    #[async_trait]
    impl EmbedPlugin for MockEmbedPlugin {
        async fn embed_batch(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _dimensions: Option<u32>,
            _items: Vec<String>,
        ) -> Result<Vec<Vec<f32>>, PluginError> {
            Ok(_items.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
        }
    }

    fn make_dispatch(tool: bool, llm: bool, embed: bool) -> PluginDispatch {
        PluginDispatch {
            tool: tool.then(|| Arc::new(MockToolPlugin) as Arc<dyn ToolPlugin>),
            llm: llm.then(|| Arc::new(MockLlmPlugin) as Arc<dyn LlmPlugin>),
            embed: embed.then(|| Arc::new(MockEmbedPlugin) as Arc<dyn EmbedPlugin>),
            tts: None,
            stt: None,
        }
    }

    fn make_tts_dispatch() -> PluginDispatch {
        PluginDispatch {
            tool: None,
            llm: None,
            embed: None,
            tts: Some(Arc::new(MockTtsPlugin) as Arc<dyn TtsPlugin>),
            stt: None,
        }
    }

    fn make_stt_dispatch() -> PluginDispatch {
        PluginDispatch {
            tool: None,
            llm: None,
            embed: None,
            tts: None,
            stt: Some(Arc::new(MockSttPlugin) as Arc<dyn SttPlugin>),
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_ok() {
        let dispatch = make_dispatch(true, true, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"key": "value"})),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            } => {
                assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
                assert_eq!(capabilities.tools, 1);
                assert_eq!(capabilities.llm_providers.len(), 1);
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_version_mismatch() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange { min: 999, max: 999 },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let dispatch = make_dispatch(false, false, false);
        let resp = dispatch_request(&dispatch, &PluginIpcRequest::Ping).await;
        assert_eq!(resp, PluginIpcResponse::Pong);
    }

    #[tokio::test]
    async fn dispatch_shutdown() {
        let dispatch = make_dispatch(false, false, false);
        let resp = dispatch_request(&dispatch, &PluginIpcRequest::Shutdown).await;
        assert_eq!(
            resp,
            PluginIpcResponse::Ack {
                request_id: String::new()
            }
        );
    }

    #[tokio::test]
    async fn dispatch_get_config_schema() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ConfigSchema { request_id, schema } => {
                assert_eq!(request_id, "req-1");
                assert!(schema.is_some());
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_get_config_schema_no_tool() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ConfigSchema { request_id, schema } => {
                assert_eq!(request_id, "req-1");
                assert!(schema.is_none());
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_tools() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::ListTools {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::Tools { request_id, tools } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(tools.len(), 1);
            }
            other => panic!("expected Tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_tools_no_tool() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::ListTools {
            request_id: "req-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::Tools { request_id, tools } => {
                assert_eq!(request_id, "req-1");
                assert!(tools.is_empty());
            }
            other => panic!("expected Tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_found() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: r#"{"msg":"hi"}"#.into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(result.unwrap().text_for_llm(), r#"{"msg":"hi"}"#);
            }
            other => panic!("expected CallResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_not_found() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "nonexistent".into(),
            arguments: "{}".into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert!(result.is_err());
            }
            other => panic!("expected CallResult with error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_no_tool_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: "{}".into(),
            deferred: false,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_call_tool_deferred_falls_back_to_sync() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::CallTool {
            request_id: "req-1".into(),
            name: "test.echo".into(),
            arguments: r#"{"msg":"hi"}"#.into(),
            deferred: true,
            context: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::CallResult { request_id, result } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(result.unwrap().text_for_llm(), r#"{"msg":"hi"}"#);
            }
            other => panic!("expected CallResult (sync fallback), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_completion() {
        let dispatch = make_dispatch(false, true, false);
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: Some(100),
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            json_schema: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::ChatCompletionResult {
                request_id,
                content,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(content, "Mock completion response");
            }
            other => panic!("expected ChatCompletionResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_completion_no_llm_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: Some(100),
            messages: vec![],
            json_schema: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_embed_batch() {
        let dispatch = make_dispatch(false, false, true);
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-2".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-embed".into(),
            dimensions: Some(3),
            items: vec!["hello".into(), "world".into()],
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::EmbedBatchResult {
                request_id,
                embeddings,
            } => {
                assert_eq!(request_id, "req-2");
                assert_eq!(embeddings.len(), 2);
                assert_eq!(embeddings[0], vec![0.1, 0.2, 0.3]);
            }
            other => panic!("expected EmbedBatchResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_embed_batch_no_embed_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "model".into(),
            dimensions: None,
            items: vec![],
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_poll_deferred_returns_unknown() {
        let dispatch = make_dispatch(true, false, false);
        let req = PluginIpcRequest::PollDeferred {
            request_id: "req-1".into(),
            task_id: "task-1".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::DeferredStatus {
                request_id,
                task_id,
                status,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(task_id, "task-1");
                assert_eq!(status, DeferredStatus::Unknown);
            }
            other => panic!("expected DeferredStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_ack_variants() {
        let dispatch = make_dispatch(true, false, false);
        let ack_requests = vec![
            PluginIpcRequest::SetCallContext {
                request_id: "req-1".into(),
                conversation_id: "c1".into(),
                turn_id: "t1".into(),
            },
            PluginIpcRequest::ApprovePermission {
                request_id: "req-1".into(),
                permission_request_id: "p1".into(),
            },
            PluginIpcRequest::AllowPattern {
                request_id: "req-1".into(),
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::RevokePattern {
                request_id: "req-1".into(),
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::CancelDeferred {
                request_id: "req-1".into(),
                task_id: "task-1".into(),
            },
        ];
        for req in &ack_requests {
            let resp = dispatch_request(&dispatch, req).await;
            assert_eq!(
                resp,
                PluginIpcResponse::Ack {
                    request_id: "req-1".into()
                },
                "expected Ack for {req:?}"
            );
        }
    }

    #[tokio::test]
    async fn chat_stream_writes_chunks_and_end() {
        let dispatch = make_dispatch(false, true, false);
        let req = PluginIpcRequest::CreateChatStream {
            request_id: "stream-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: None,
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            tools: vec![],
        };

        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        handle_request(&dispatch, &req, &mut server).await.unwrap();
        drop(server);

        let mut responses = Vec::new();
        loop {
            let resp = ene_plugin_proto::read_plugin_response(&mut client)
                .await
                .unwrap()
                .unwrap();
            let is_terminal = matches!(
                resp,
                PluginIpcResponse::StreamEnd { .. } | PluginIpcResponse::StreamError { .. }
            );
            responses.push(resp);
            if is_terminal {
                break;
            }
        }

        assert_eq!(responses.len(), 3);
        assert!(
            matches!(&responses[0], PluginIpcResponse::StreamChunk { text_delta, .. } if text_delta == "Hello")
        );
        assert!(
            matches!(&responses[1], PluginIpcResponse::StreamChunk { text_delta, .. } if text_delta == " world")
        );
        assert!(
            matches!(&responses[2], PluginIpcResponse::StreamEnd { request_id } if request_id == "stream-1")
        );
    }

    #[tokio::test]
    async fn dispatch_empty_dispatch_returns_default_capabilities() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::Handshake {
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck { capabilities, .. } => {
                assert_eq!(capabilities.tools, 0);
                assert!(capabilities.llm_providers.is_empty());
                assert!(capabilities.tts_providers.is_empty());
                assert!(capabilities.stt_providers.is_empty());
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_synthesize_speech_ok() {
        let dispatch = make_tts_dispatch();
        let req = PluginIpcRequest::SynthesizeSpeech {
            request_id: "req-tts-1".into(),
            provider_kind: "mock_tts".into(),
            provider_config: serde_json::json!({}),
            text: "Hello world".into(),
            voice: "default".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::SpeechResult {
                request_id,
                audio_base64,
                format,
            } => {
                assert_eq!(request_id, "req-tts-1");
                assert!(!audio_base64.is_empty());
                assert_eq!(format, "wav");
            }
            other => panic!("expected SpeechResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_synthesize_speech_no_tts_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::SynthesizeSpeech {
            request_id: "req-tts-1".into(),
            provider_kind: "mock_tts".into(),
            provider_config: serde_json::json!({}),
            text: "Hello".into(),
            voice: "default".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_ok() {
        let dispatch = make_stt_dispatch();
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: base64_encode(b"fake audio"),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        match resp {
            PluginIpcResponse::TranscriptionResult { request_id, text } => {
                assert_eq!(request_id, "req-stt-1");
                assert_eq!(text, "Mock transcription");
            }
            other => panic!("expected TranscriptionResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_no_stt_returns_error() {
        let dispatch = make_dispatch(false, false, false);
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: "AAAA".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_transcribe_audio_invalid_base64_returns_error() {
        let dispatch = make_stt_dispatch();
        let req = PluginIpcRequest::TranscribeAudio {
            request_id: "req-stt-1".into(),
            provider_kind: "mock_stt".into(),
            provider_config: serde_json::json!({}),
            audio_base64: "!!!invalid base64!!!".into(),
            format: "wav".into(),
        };
        let resp = dispatch_request(&dispatch, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }
}
