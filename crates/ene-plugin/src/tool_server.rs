use ene_plugin_proto::{
    DeferredOutcome, IPC_PROTOCOL_VERSION, IpcListener, IpcRequest, IpcResponse, ToolError,
    ToolProvider, cleanup_path, read_ipc_request, write_ipc_response,
};
use parking_lot::Mutex;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
///     ene_plugin::run_tool_server(Box::new(provider)).await;
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
    let call_mutex: Arc<Mutex<()>> = Arc::new(Mutex::new(()));
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
                let mut stream = result?;
                let provider = Arc::clone(&provider);
                let call_mutex = Arc::clone(&call_mutex);
                let shutdown = Arc::clone(&shutdown);
                let tasks = Arc::clone(&tasks);
                let handle = tokio::spawn(async move {
                    loop {
                        match read_ipc_request(&mut stream).await {
                            Ok(None) => break,
                            Ok(Some(req)) => {
                                let is_shutdown = matches!(req, IpcRequest::Shutdown);
                                let resp = dispatch(provider.as_ref(), &call_mutex, &req).await;
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
                                drop(write_ipc_response(
                                    &mut stream,
                                    &IpcResponse::Error { message: e.to_string() },
                                )
                                .await);
                                break;
                            }
                        }
                    }
                });
                tasks.lock().await.push(handle);
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

    // Join all spawned connection tasks before cleanup. This prevents
    // orphaned tasks from writing to a closed socket or holding
    // references to the provider Arc after the server has exited.
    {
        let mut guard = tasks.lock().await;
        let handles: Vec<_> = guard.drain(..).collect();
        drop(guard);
        for handle in handles {
            drop(tokio::time::timeout(Duration::from_secs(5), handle).await);
        }
    }

    cleanup_path(&socket_path);
    tracing::info!(component = "ToolServer", "Shutting down");
    Ok(())
}

#[expect(
    clippy::await_holding_lock,
    reason = "Intentionally held across await to serialize set_call_context + tool call; parking_lot::Mutex with send_guard feature is Send-safe across await"
)]
async fn dispatch(
    provider: &dyn ToolProvider,
    call_mutex: &Mutex<()>,
    req: &IpcRequest,
) -> IpcResponse {
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
            // Serialize sandbox/config application under the same lock that
            // guards `set_call_context` + `call_tool`, so a handshake on one
            // connection cannot interleave its setter writes with a tool call
            // already in flight on another connection sharing this provider.
            // These setters are synchronous, so the (parking_lot) guard is
            // never held across an `.await`.
            {
                let _lock = call_mutex.lock();
                provider.set_sandbox(sandbox);
                if let Some(config) = tool_config {
                    provider.set_config(config);
                }
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
        IpcRequest::CallTool {
            name,
            arguments,
            deferred: true,
            context,
        } => {
            let _lock = call_mutex.lock();
            if let Some(ctx) = context {
                provider.set_call_context(ctx);
            }
            match provider.call_tool_deferred(name, arguments).await {
                Ok(DeferredOutcome::Sync(result)) => IpcResponse::CallResult {
                    result: Ok(result.text_for_llm()),
                },
                Ok(DeferredOutcome::Deferred { task_id }) => {
                    IpcResponse::DeferredAccepted { task_id }
                }
                Err(e) => IpcResponse::CallResult { result: Err(e) },
            }
        }
        IpcRequest::CallTool {
            name,
            arguments,
            deferred: false,
            context,
        } => {
            let _lock = call_mutex.lock();
            if let Some(ctx) = context {
                provider.set_call_context(ctx);
            }
            match provider.call_tool(name, arguments).await {
                Ok(result) => IpcResponse::CallResult { result: Ok(result) },
                Err(e) => IpcResponse::CallResult { result: Err(e) },
            }
        }
        IpcRequest::SetCallContext {
            conversation_id: _,
            turn_id: _,
        } => {
            tracing::warn!(
                component = "ToolServer",
                "SetCallContext is deprecated; per-call context is now passed directly \
                 in CallTool requests"
            );
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
        IpcRequest::RevokePattern {
            action,
            target_pattern,
        } => {
            provider.revoke_pattern(action, target_pattern);
            IpcResponse::Ack
        }
        IpcRequest::Shutdown => IpcResponse::Ack,
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::PollDeferred { task_id } => IpcResponse::DeferredStatus {
            task_id: task_id.clone(),
            status: provider.poll_deferred(task_id),
        },
        IpcRequest::CancelDeferred { task_id } => {
            provider.cancel_deferred(task_id);
            IpcResponse::Ack
        }
    }
}
