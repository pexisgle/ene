use crate::transport::{IpcListener, cleanup_path};
use crate::{
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, ToolProvider, read_ipc_request,
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
/// # Usage Example
///
/// ```ignore
/// #[tokio::main]
/// async fn main() {
///     let provider = MyToolProvider::new();
///     ene_tool_proto::run_tool_server(Box::new(provider)).await;
/// }
/// ```
pub async fn run_tool_server(
    provider: Box<dyn ToolProvider>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    tracing::info!("[tool-server] Listening on {}", socket_path.display());

    let provider: Arc<dyn ToolProvider> = provider.into();
    let shutdown = Arc::new(tokio::sync::Notify::new());

    loop {
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
                                    tracing::error!("[tool-server] Failed to write response: {e}");
                                    break;
                                }
                                if is_shutdown {
                                    shutdown.notify_one();
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::error!("[tool-server] IPC read error: {e}");
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
        }
    }

    cleanup_path(&socket_path);
    tracing::info!("[tool-server] Shutting down");
    Ok(())
}

async fn dispatch(provider: &dyn ToolProvider, req: &IpcRequest) -> IpcResponse {
    match req {
        IpcRequest::Handshake { version } => {
            let agreed = (*version).min(IPC_PROTOCOL_VERSION);
            IpcResponse::HandshakeAck { version: agreed }
        }
        IpcRequest::Initialize {
            sandbox,
            tool_config,
        } => {
            provider.set_sandbox(sandbox);
            if let Some(config) = tool_config {
                provider.set_config(config);
            }
            IpcResponse::Ack
        }
        IpcRequest::GetConfigSchema => IpcResponse::ConfigSchema {
            schema: provider.config_schema(),
        },
        IpcRequest::ListTools => IpcResponse::Tools {
            tools: provider.list_specs(),
        },
        IpcRequest::ListActionSpecs => IpcResponse::ActionSpecs {
            specs: provider.list_action_specs(),
        },
        IpcRequest::CallTool { name, arguments } => match provider.call_tool(name, arguments).await
        {
            Ok(result) => IpcResponse::CallResult { result: Ok(result) },
            Err(e) => IpcResponse::CallResult { result: Err(e) },
        },
        IpcRequest::SetSessionId { session_id } => {
            provider.set_session_id(session_id);
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
        IpcRequest::GetMyConfig => IpcResponse::MyConfig(provider.get_config()),
        IpcRequest::SetMyConfig(config) => {
            provider.set_config(config);
            IpcResponse::Ack
        }
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Shutdown => IpcResponse::Ack,
    }
}
