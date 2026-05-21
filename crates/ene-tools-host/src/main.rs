mod registry;

use ene_tool_proto::{read_ipc_request, write_ipc_response, IpcRequest, IpcResponse};
use registry::HostRegistry;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket_path = std::env::var("ENE_TOOL_HOST_SOCKET")
        .unwrap_or_else(|_| "/tmp/ene-tool-host.sock".to_string());
    let socket_path = PathBuf::from(socket_path);

    // 既存のソケットファイルを削除
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    println!("[host] Listening on {}", socket_path.display());

    let mut registry = HostRegistry::new();
    registry.add_provider(Box::new(ene_tools_utility::provider::UtilityToolProvider::new()));
    registry.add_provider(Box::new(ene_tools_web::provider::WebToolProvider));
    registry.add_provider(Box::new(ene_tools_app::provider::AppToolProvider));
    registry.add_provider(Box::new(ene_tools_browser::provider::BrowserToolProvider::new()));
    registry.add_provider(Box::new(ene_tools_fs::provider::FsToolProvider));
    let registry = Arc::new(registry);

    loop {
        let (mut stream, _) = listener.accept().await?;
        println!("[host] Client connected");

        let registry = Arc::clone(&registry);
        tokio::spawn(async move {
            loop {
                match read_ipc_request(&mut stream).await {
                    Ok(None) => {
                        println!("[host] Client disconnected");
                        break;
                    }
                    Ok(Some(req)) => {
                        let is_shutdown = matches!(req, IpcRequest::Shutdown);
                        let resp = dispatch(&registry, &req).await;
                        if let Err(e) = write_ipc_response(&mut stream, &resp).await {
                            eprintln!("[host] Failed to write response: {e}");
                            break;
                        }
                        // Shutdown 受信後はグレースフルに終了
                        if is_shutdown {
                            println!("[host] Shutting down");
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("[host] IPC read error: {e}");
                        let _ = write_ipc_response(
                            &mut stream,
                            &IpcResponse::Error {
                                message: e.to_string(),
                            },
                        )
                        .await;
                        break;
                    }
                }
            }
        });
    }
}

async fn dispatch(registry: &HostRegistry, req: &IpcRequest) -> IpcResponse {
    match req {
        IpcRequest::Initialize { sandbox } => {
            println!(
                "[host] Initialize received (sandbox enabled: {})",
                sandbox.enabled
            );
            IpcResponse::Ack
        }
        IpcRequest::ListTools => {
            let tools = registry.list_tools();
            IpcResponse::Tools { tools }
        }
        IpcRequest::CallTool { name, arguments } => match registry.call_tool(&name, &arguments).await
        {
            Ok(result) => IpcResponse::CallResult {
                result: Ok(result),
            },
            Err(e) => IpcResponse::CallResult { result: Err(e) },
        },
        IpcRequest::SetSessionId { session_id } => {
            registry.set_session_id(&session_id);
            IpcResponse::Ack
        }
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Shutdown => {
            println!("[host] Shutdown requested");
            IpcResponse::Ack
        }
    }
}
