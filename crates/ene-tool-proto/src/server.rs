use crate::transport::{IpcListener, cleanup_path};
use crate::{
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, ToolError, ToolProvider, read_ipc_request,
    write_ipc_response,
};
use std::path::PathBuf;
use std::sync::Arc;

/// Starts a tool provider as an IPC server
///
/// Reads the socket path from the `ENE_TOOL_SOCKET` environment variable
/// and listens for requests over IPC.
/// Shuts down upon receiving a `Shutdown` request.
///
/// Returns [`ToolError`] rather
/// than a boxed error trait object, so callers that want to `match` on the
/// failure (e.g. distinguishing a bind failure from a transport error) can
/// do so without downcasting. `ToolError` already has a `From<io::Error>`
/// impl, so the socket bind/accept/permission calls inside this function
/// convert transparently via `?`.
///
/// # Usage Example
///
/// ```ignore
/// #[tokio::main]
/// async fn main() {
///     let provider = MyToolProvider::new();
///     ene_tool_proto::run_tool_server(Box::new(provider)).await;
/// }
/// ```
pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), ToolError> {
    let socket_path = std::env::var("ENE_TOOL_SOCKET").unwrap_or_else(|_| {
        #[cfg(unix)]
        {
            "/tmp/ene-tool.sock".to_string()
        }
        #[cfg(windows)]
        {
            r"\\.\pipe\ene-tool".to_string()
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

    tracing::info!(component = "ToolServer", socket = %socket_path.display(), "Listening");

    let provider: Arc<dyn ToolProvider> = provider.into();
    let shutdown = Arc::new(tokio::sync::Notify::new());

    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        #[cfg(unix)]
        let sigterm_fut = sigterm.recv();
        #[cfg(not(unix))]
        let sigterm_fut = std::future::pending::<()>();

        tokio::select! {
            result = listener.accept() => {
                let mut stream = result?;
                let provider = Arc::clone(&provider);
                let shutdown = Arc::clone(&shutdown);
                tokio::spawn(async move {
                    loop {
                        match read_ipc_request(&mut stream).await {
                            Ok(None) => break,
                            Ok(Some(req)) => {
                                let is_shutdown = matches!(req, IpcRequest::Shutdown);
                                let resp = dispatch(provider.as_ref(), &req).await;
                                if let Err(e) = write_ipc_response(&mut stream, &resp).await {
                                    tracing::error!(component = "ToolServer", error = %e, "Failed to write response");
                                    break;
                                }
                                if is_shutdown {
                                    shutdown.notify_one();
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::error!(component = "ToolServer", error = %e, "IPC read error");
                                let _ = write_ipc_response(
                                    &mut stream,
                                    &IpcResponse::Error { message: e.to_string() },
                                )
                                .await;
                                break;
                            }
                        }
                    }
                });
            }
            () = shutdown.notified() => {
                break;
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!(component = "ToolServer", "Received SIGINT, shutting down");
                break;
            }
            _ = sigterm_fut => {
                tracing::info!(component = "ToolServer", "Received SIGTERM, shutting down");
                break;
            }
        }
    }

    cleanup_path(&socket_path);
    tracing::info!(component = "ToolServer", "Shutting down");
    Ok(())
}

async fn dispatch(provider: &dyn ToolProvider, req: &IpcRequest) -> IpcResponse {
    match req {
        IpcRequest::Handshake {
            version,
            sandbox,
            tool_config,
        } => {
            if *version != IPC_PROTOCOL_VERSION {
                tracing::error!(
                    "[tool-server] Handshake version mismatch: client sent {version}, server requires {IPC_PROTOCOL_VERSION}"
                );
                return IpcResponse::Error {
                    message: format!(
                        "protocol version mismatch: client sent {version}, server requires {IPC_PROTOCOL_VERSION}"
                    ),
                };
            }
            provider.set_sandbox(sandbox);
            if let Some(config) = tool_config {
                provider.set_config(config);
            }
            IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            }
        }
        IpcRequest::GetConfigSchema => IpcResponse::ConfigSchema {
            schema: provider.config_schema(),
        },
        IpcRequest::ListTools => IpcResponse::Tools {
            tools: provider.list_specs(),
        },
        IpcRequest::ListRagProfiles => IpcResponse::RagProfiles {
            profiles: provider.list_rag_profiles(),
        },
        IpcRequest::CallTool { name, arguments } => match provider.call_tool(name, arguments).await
        {
            Ok(result) => IpcResponse::CallResult { result: Ok(result) },
            Err(e) => IpcResponse::CallResult { result: Err(e) },
        },
        IpcRequest::SetCallContext {
            conversation_id,
            turn_id,
        } => {
            provider.set_call_context(&crate::CallContext {
                conversation_id: conversation_id.clone(),
                turn_id: turn_id.clone(),
            });
            IpcResponse::Ack
        }
        IpcRequest::ApprovePermission { request_id } => {
            provider.approve_permission(request_id);
            IpcResponse::Ack
        }
        IpcRequest::AllowPattern {
            action,
            target_pattern,
        } => {
            provider.allow_pattern(action, target_pattern);
            IpcResponse::Ack
        }
        IpcRequest::Shutdown => IpcResponse::Ack,
    }
}
