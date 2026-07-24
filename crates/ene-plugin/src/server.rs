//! Plugin IPC server: accept loop, request dispatch, and streaming handler.
//!
//! [`run_plugin_server`] is the entry point for plugin binaries. It binds an
//! IPC listener, accepts connections, and dispatches
//! [`PluginIpcRequest`](ene_plugin_proto::PluginIpcRequest) messages to the
//! [`Plugin`](crate::Plugin) implementation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ene_plugin_proto::DeferredStatus;
use ene_plugin_proto::{
    IpcListener, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginError, PluginIpcRequest,
    PluginIpcResponse, cleanup_path, read_plugin_request, write_plugin_response,
};
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::plugin::Plugin;

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
/// # use ene_plugin::{Plugin, PluginCapabilities, PluginError};
/// # struct MyPlugin;
/// # #[async_trait::async_trait]
/// # impl Plugin for MyPlugin {
/// #     fn capabilities(&self) -> PluginCapabilities { PluginCapabilities::default() }
/// # }
/// #[tokio::main]
/// async fn main() -> Result<(), PluginError> {
///     ene_plugin::run_plugin_server(Box::new(MyPlugin)).await
/// }
/// ```
pub async fn run_plugin_server(plugin: Box<dyn Plugin>) -> Result<(), PluginError> {
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

    let plugin: Arc<dyn Plugin> = plugin.into();
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
                let plugin = Arc::clone(&plugin);
                let shutdown = Arc::clone(&shutdown);
                let tasks = Arc::clone(&tasks);
                let handle = tokio::spawn(async move {
                    handle_connection(plugin, stream, shutdown).await;
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
    // to the plugin Arc after the server has exited.
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
    plugin: Arc<dyn Plugin>,
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
                        message: e.to_string(),
                    },
                )
                .await;
                break;
            }
        };

        let is_shutdown = matches!(req, PluginIpcRequest::Shutdown);

        if let Err(e) = handle_request(plugin.as_ref(), &req, &mut stream).await {
            tracing::error!(
                component = "PluginServer",
                error = %e,
                "Failed to handle request"
            );
            break;
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
    plugin: &dyn Plugin,
    req: &PluginIpcRequest,
    writer: &mut W,
) -> Result<(), PluginError> {
    if matches!(req, PluginIpcRequest::CreateChatStream { .. }) {
        handle_chat_stream(plugin, req, writer).await
    } else {
        let resp = dispatch(plugin, req).await;
        write_plugin_response(writer, &resp).await
    }
}

/// Dispatches a non-streaming request and returns the single response.
async fn dispatch(plugin: &dyn Plugin, req: &PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version,
            sandbox,
            plugin_config,
        } => {
            if *version != PLUGIN_IPC_PROTOCOL_VERSION {
                tracing::error!(
                    component = "PluginServer",
                    client_version = version,
                    server_version = PLUGIN_IPC_PROTOCOL_VERSION,
                    "Handshake version mismatch"
                );
                return PluginIpcResponse::Error {
                    message: format!(
                        "protocol version mismatch: client sent {version}, \
                         server requires {PLUGIN_IPC_PROTOCOL_VERSION}"
                    ),
                };
            }
            plugin.set_sandbox(sandbox);
            if let Some(config) = plugin_config {
                plugin.set_config(config);
            }
            PluginIpcResponse::HandshakeAck {
                version: PLUGIN_IPC_PROTOCOL_VERSION,
                capabilities: plugin.capabilities(),
            }
        }
        PluginIpcRequest::Ping => PluginIpcResponse::Pong,
        PluginIpcRequest::GetConfigSchema => PluginIpcResponse::ConfigSchema {
            schema: plugin.config_schema(),
        },
        PluginIpcRequest::ListTools => PluginIpcResponse::Tools {
            tools: plugin.list_tool_specs(),
        },
        PluginIpcRequest::CallTool {
            name, arguments, ..
        } => {
            let result = plugin.call_tool(name, arguments).await;
            PluginIpcResponse::CallResult { result }
        }
        PluginIpcRequest::ChatCompletion {
            request_id,
            provider_kind,
            provider_config,
            model,
            max_tokens,
            messages,
            json_schema,
        } => match plugin
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
                message: e.to_string(),
            },
        },
        PluginIpcRequest::EmbedBatch {
            request_id,
            provider_kind,
            provider_config,
            model,
            dimensions,
            items,
        } => {
            let items: Vec<(String, String)> = items
                .iter()
                .map(|text| (String::new(), text.clone()))
                .collect();
            match plugin
                .embed_batch(
                    provider_kind,
                    provider_config.clone(),
                    model.clone(),
                    dimensions.map(|d| d as usize),
                    items,
                )
                .await
            {
                Ok(embeddings) => PluginIpcResponse::EmbedBatchResult {
                    request_id: request_id.clone(),
                    embeddings,
                },
                Err(e) => PluginIpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::PollDeferred { task_id } => PluginIpcResponse::DeferredStatus {
            task_id: task_id.clone(),
            status: DeferredStatus::Unknown,
        },
        PluginIpcRequest::SetCallContext { .. }
        | PluginIpcRequest::ApprovePermission { .. }
        | PluginIpcRequest::AllowPattern { .. }
        | PluginIpcRequest::RevokePattern { .. }
        | PluginIpcRequest::CancelDeferred { .. }
        | PluginIpcRequest::Shutdown => PluginIpcResponse::Ack,
        // CreateChatStream is handled by handle_chat_stream, not here.
        PluginIpcRequest::CreateChatStream { .. } => PluginIpcResponse::Error {
            message: "CreateChatStream must be handled by the streaming path".to_string(),
        },
    }
}

/// Handles a `CreateChatStream` request by iterating the plugin's stream
/// and writing `StreamChunk` / `StreamEnd` / `StreamError` responses.
async fn handle_chat_stream<W: AsyncWriteExt + Unpin>(
    plugin: &dyn Plugin,
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

    match plugin
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
            write_plugin_response(
                writer,
                &PluginIpcResponse::StreamError {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            )
            .await
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
    use crate::plugin::PluginStreamChunk;
    use async_trait::async_trait;
    use ene_plugin_proto::ToolName;
    use ene_plugin_proto::{LlmProviderSpec, PluginCapabilities, ToolSpec};

    /// A mock plugin that returns fixed responses for testing dispatch logic.
    struct MockPlugin;

    #[async_trait]
    impl Plugin for MockPlugin {
        fn capabilities(&self) -> PluginCapabilities {
            PluginCapabilities {
                tools: vec![ToolSpec::new(
                    ToolName::new("test.echo"),
                    "Echoes the input arguments.",
                    serde_json::json!({}),
                )],
                llm_providers: vec![LlmProviderSpec {
                    kind: "mock".into(),
                    supported_models: vec!["mock-model".into()],
                    supports_streaming: true,
                    supports_vision: false,
                }],
                tts_providers: vec![],
                stt_providers: vec![],
            }
        }

        async fn call_tool(
            &self,
            name: &str,
            args: &str,
        ) -> Result<String, ene_plugin_proto::ToolError> {
            if name == "test.echo" {
                Ok(args.to_string())
            } else {
                Err(ene_plugin_proto::ToolError::NotFound {
                    tool_name: name.to_string(),
                })
            }
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

        async fn embed_batch(
            &self,
            _kind: &str,
            _config: serde_json::Value,
            _model: String,
            _dimensions: Option<usize>,
            items: Vec<(String, String)>,
        ) -> Result<Vec<Vec<f32>>, PluginError> {
            Ok(items.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
        }

        fn config_schema(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({"type": "object"}))
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_ok() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::Handshake {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"key": "value"})),
        };
        let resp = dispatch(&plugin, &req).await;
        match resp {
            PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            } => {
                assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
                assert_eq!(capabilities.tools.len(), 1);
                assert_eq!(capabilities.llm_providers.len(), 1);
            }
            other => panic!("expected HandshakeAck, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_handshake_version_mismatch() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::Handshake {
            version: 999,
            sandbox: ene_plugin_proto::SandboxConfigData::default(),
            plugin_config: None,
        };
        let resp = dispatch(&plugin, &req).await;
        assert!(matches!(resp, PluginIpcResponse::Error { .. }));
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let plugin = MockPlugin;
        let resp = dispatch(&plugin, &PluginIpcRequest::Ping).await;
        assert_eq!(resp, PluginIpcResponse::Pong);
    }

    #[tokio::test]
    async fn dispatch_shutdown() {
        let plugin = MockPlugin;
        let resp = dispatch(&plugin, &PluginIpcRequest::Shutdown).await;
        assert_eq!(resp, PluginIpcResponse::Ack);
    }

    #[tokio::test]
    async fn dispatch_get_config_schema() {
        let plugin = MockPlugin;
        let resp = dispatch(&plugin, &PluginIpcRequest::GetConfigSchema).await;
        match resp {
            PluginIpcResponse::ConfigSchema { schema } => {
                assert!(schema.is_some());
            }
            other => panic!("expected ConfigSchema, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_list_tools() {
        let plugin = MockPlugin;
        let resp = dispatch(&plugin, &PluginIpcRequest::ListTools).await;
        match resp {
            PluginIpcResponse::Tools { tools } => {
                assert_eq!(tools.len(), 1);
            }
            other => panic!("expected Tools, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_found() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::CallTool {
            name: "test.echo".into(),
            arguments: r#"{"msg":"hi"}"#.into(),
            deferred: false,
        };
        let resp = dispatch(&plugin, &req).await;
        match resp {
            PluginIpcResponse::CallResult { result } => {
                assert_eq!(result.unwrap(), r#"{"msg":"hi"}"#);
            }
            other => panic!("expected CallResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_call_tool_not_found() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::CallTool {
            name: "nonexistent".into(),
            arguments: "{}".into(),
            deferred: false,
        };
        let resp = dispatch(&plugin, &req).await;
        match resp {
            PluginIpcResponse::CallResult { result } => {
                assert!(result.is_err());
            }
            other => panic!("expected CallResult with error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_completion() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::ChatCompletion {
            request_id: "req-1".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-model".into(),
            max_tokens: Some(100),
            messages: vec![serde_json::json!({"role": "user", "content": "Hi"})],
            json_schema: None,
        };
        let resp = dispatch(&plugin, &req).await;
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
    async fn dispatch_embed_batch() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::EmbedBatch {
            request_id: "req-2".into(),
            provider_kind: "mock".into(),
            provider_config: serde_json::json!({}),
            model: "mock-embed".into(),
            dimensions: Some(3),
            items: vec!["hello".into(), "world".into()],
        };
        let resp = dispatch(&plugin, &req).await;
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
    async fn dispatch_poll_deferred_returns_unknown() {
        let plugin = MockPlugin;
        let req = PluginIpcRequest::PollDeferred {
            task_id: "task-1".into(),
        };
        let resp = dispatch(&plugin, &req).await;
        match resp {
            PluginIpcResponse::DeferredStatus { task_id, status } => {
                assert_eq!(task_id, "task-1");
                assert_eq!(status, DeferredStatus::Unknown);
            }
            other => panic!("expected DeferredStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_ack_variants() {
        let plugin = MockPlugin;
        let ack_requests = vec![
            PluginIpcRequest::SetCallContext {
                conversation_id: "c1".into(),
                turn_id: "t1".into(),
            },
            PluginIpcRequest::ApprovePermission {
                request_id: "p1".into(),
            },
            PluginIpcRequest::AllowPattern {
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::RevokePattern {
                action: "fs_write".into(),
                target_pattern: "/tmp/**".into(),
            },
            PluginIpcRequest::CancelDeferred {
                task_id: "task-1".into(),
            },
        ];
        for req in &ack_requests {
            let resp = dispatch(&plugin, req).await;
            assert_eq!(resp, PluginIpcResponse::Ack, "expected Ack for {req:?}");
        }
    }

    #[tokio::test]
    async fn chat_stream_writes_chunks_and_end() {
        let plugin = MockPlugin;
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
        handle_request(&plugin, &req, &mut server).await.unwrap();
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
    async fn default_plugin_returns_not_supported() {
        struct EmptyPlugin;

        #[async_trait]
        impl Plugin for EmptyPlugin {
            fn capabilities(&self) -> PluginCapabilities {
                PluginCapabilities::default()
            }
        }

        let plugin = EmptyPlugin;
        let stream_result = plugin
            .create_chat_stream(
                "any",
                serde_json::json!({}),
                "model".into(),
                None,
                vec![],
                vec![],
            )
            .await;
        assert!(matches!(stream_result, Err(PluginError::NotSupported(_))));

        let completion_result = plugin
            .chat_completion(
                "any",
                serde_json::json!({}),
                "model".into(),
                None,
                vec![],
                None,
            )
            .await;
        assert!(matches!(
            completion_result,
            Err(PluginError::NotSupported(_))
        ));

        let embed_result = plugin
            .embed_batch("any", serde_json::json!({}), "model".into(), None, vec![])
            .await;
        assert!(matches!(embed_result, Err(PluginError::NotSupported(_))));

        let tool_result = plugin.call_tool("anything", "{}").await;
        assert!(tool_result.is_err());
    }
}
