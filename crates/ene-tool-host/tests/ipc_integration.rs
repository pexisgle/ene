#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests use unwrap/expect for assertions"
)]
#![expect(
    clippy::panic,
    reason = "integration tests use panic! for assertion failures"
)]

use ene_tool_host::ToolRegistry;
use ene_tool_proto::transport::IpcListener;
use ene_tool_proto::{
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, SandboxConfigData, ToolName, ToolSpec,
    read_ipc_request, write_ipc_response,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// End-to-end test of the IPC protocol
///
/// Sets up a mock IPC server and verifies the full flow from `IpcToolRegistry`
/// connection through Handshake, `ListTools`, and `CallTool`
#[tokio::test]
async fn ipc_e2e_handshake_list_tools_and_call_tool() {
    let socket_path: PathBuf = {
        #[cfg(unix)]
        {
            let p = PathBuf::from("/tmp/ene-test-e2e.sock");
            ene_tool_proto::transport::cleanup_path(&p);
            p
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"\\.\pipe\ene-test-e2e")
        }
    };

    let mut listener = IpcListener::bind(&socket_path).unwrap();

    // Mock server task
    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await.unwrap();

        // 1. Handshake
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("server sends a Handshake request");
        assert!(
            matches!(
                req,
                IpcRequest::Handshake {
                    version: IPC_PROTOCOL_VERSION,
                    ..
                }
            ),
            "Expected Handshake, got {req:?}"
        );
        write_ipc_response(
            &mut stream,
            &IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            },
        )
        .await
        .unwrap();

        // 2. ListTools (refresh_tools is called inside IpcToolRegistry::new)
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("server sends a ListTools request");
        assert!(
            matches!(req, IpcRequest::ListTools),
            "Expected ListTools, got {req:?}"
        );
        write_ipc_response(
            &mut stream,
            &IpcResponse::Tools {
                tools: vec![ToolSpec::new(
                    ToolName::new("utility.get_current_time"),
                    "Get the current date and time.",
                    serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                )],
            },
        )
        .await
        .unwrap();

        // 3. ListRagProfiles (also part of refresh_tools in IpcToolRegistry::new)
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("server sends a ListRagProfiles request");
        assert!(
            matches!(req, IpcRequest::ListRagProfiles),
            "Expected ListRagProfiles, got {req:?}"
        );
        write_ipc_response(
            &mut stream,
            &IpcResponse::RagProfiles {
                profiles: Vec::new(),
            },
        )
        .await
        .unwrap();

        // 4. CallTool
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("server sends a CallTool request");
        match &req {
            IpcRequest::CallTool { name, arguments } => {
                assert_eq!(name, "utility.get_current_time");
                assert_eq!(arguments, "{}");
            }
            other => assert!(
                matches!(other, IpcRequest::CallTool { .. }),
                "Expected CallTool, got {other:?}"
            ),
        }
        write_ipc_response(
            &mut stream,
            &IpcResponse::CallResult {
                result: Ok("2024-01-01 12:00:00".to_string()),
            },
        )
        .await
        .unwrap();
    });

    // Client side — connects using IpcToolRegistry
    let sandbox = SandboxConfigData {
        enabled: true,
        allowed_directories: vec![".".to_string()],
        writable_directories: vec![".".to_string()],
        blocked_commands: vec![],
        max_read_bytes: 50 * 1024,
        max_write_bytes: 1024 * 1024,
        shell_timeout_ms: 120_000,
        max_shell_output_bytes: 50 * 1024,
        max_shell_output_lines: 2000,
        db_socket: None,
        db_auth_token: None,
    };

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, sandbox, None, 60_000)
        .await
        .expect("socket server is reachable");

    // Verify list_tools
    let tools = registry.list_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(
        tools.first().map(|tool| tool.name.as_str()),
        Some("utility.get_current_time")
    );

    // Verify call_tool
    let result = registry
        .call_tool("utility.get_current_time", "{}")
        .await
        .expect("tool executes successfully");
    assert_eq!(result, "2024-01-01 12:00:00");

    // Wait for server to shut down
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("Server timed out")
        .expect("Server panicked");
}

/// Spins up a mock IPC server that completes the handshake + tool refresh,
/// then hands the stream to `on_connected` for request-specific behavior.
fn spawn_mock_server(
    socket_path: &Path,
    on_connected: impl FnOnce(ene_tool_proto::transport::IpcStream) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let mut listener = IpcListener::bind(socket_path).unwrap();
    tokio::spawn(async move {
        let mut stream = listener.accept().await.unwrap();

        // Handshake
        let req = read_ipc_request(&mut stream).await.unwrap().unwrap();
        assert!(matches!(req, IpcRequest::Handshake { .. }));
        write_ipc_response(
            &mut stream,
            &IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            },
        )
        .await
        .unwrap();

        // ListTools
        let req = read_ipc_request(&mut stream).await.unwrap().unwrap();
        assert!(matches!(req, IpcRequest::ListTools));
        write_ipc_response(
            &mut stream,
            &IpcResponse::Tools {
                tools: vec![ToolSpec::new(
                    ToolName::new("test.echo"),
                    "echo",
                    serde_json::json!({}),
                )],
            },
        )
        .await
        .unwrap();

        // ListRagProfiles
        let req = read_ipc_request(&mut stream).await.unwrap().unwrap();
        assert!(matches!(req, IpcRequest::ListRagProfiles));
        write_ipc_response(
            &mut stream,
            &IpcResponse::RagProfiles {
                profiles: Vec::new(),
            },
        )
        .await
        .unwrap();

        on_connected(stream);
    })
}

fn test_socket(tag: &str) -> PathBuf {
    #[cfg(unix)]
    {
        let p = PathBuf::from(format!("/tmp/ene-test-{tag}.sock"));
        ene_tool_proto::transport::cleanup_path(&p);
        p
    }
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\ene-test-{tag}"))
    }
}

fn test_sandbox() -> SandboxConfigData {
    SandboxConfigData {
        enabled: true,
        allowed_directories: vec![".".to_string()],
        writable_directories: vec![".".to_string()],
        blocked_commands: vec![],
        max_read_bytes: 50 * 1024,
        max_write_bytes: 1024 * 1024,
        shell_timeout_ms: 120_000,
        max_shell_output_bytes: 50 * 1024,
        max_shell_output_lines: 2000,
        db_socket: None,
        db_auth_token: None,
    }
}

/// A responsive tool answers `Ping` with `Pong`, so the liveness probe
/// reports healthy (#238).
#[tokio::test]
async fn ping_responsive_tool_is_healthy() {
    let socket_path = test_socket("ping-ok");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            while let Ok(Some(IpcRequest::Ping)) = read_ipc_request(&mut stream).await {
                write_ipc_response(&mut stream, &IpcResponse::Pong)
                    .await
                    .unwrap();
            }
        });
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    registry.ping().await.expect("responsive tool answers Ping");

    server.abort();
}

/// A hung tool that never answers `Ping` is detected as unhealthy: the
/// probe times out instead of blocking forever (#238).
#[tokio::test]
async fn ping_hung_tool_times_out() {
    let socket_path = test_socket("ping-hang");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            // Read requests but never respond, simulating a hang.
            while let Ok(Some(_)) = read_ipc_request(&mut stream).await {}
        });
    });

    // Short per-call timeout so the probe fails fast in the test.
    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 200)
        .await
        .expect("socket server is reachable");

    let result = tokio::time::timeout(Duration::from_secs(5), registry.ping()).await;
    match result {
        Ok(Err(_)) => {} // Expected: probe surfaced a timeout/transport error.
        Ok(Ok(())) => panic!("hung tool should not answer Ping"),
        Err(elapsed) => panic!("ping probe itself hung past the test timeout: {elapsed}"),
    }

    server.abort();
}
