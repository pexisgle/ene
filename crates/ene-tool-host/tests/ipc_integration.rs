#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests use unwrap/expect for assertions"
)]

use ene_tool_host::ToolRegistry;
use ene_tool_proto::transport::IpcListener;
use ene_tool_proto::{
    IPC_PROTOCOL_VERSION, IpcRequest, IpcResponse, SandboxConfigData, ToolName, ToolSpec,
    read_ipc_request, write_ipc_response,
};
use std::path::PathBuf;
use std::time::Duration;

/// End-to-end test of the IPC protocol
///
/// Sets up a mock IPC server and verifies the full flow from `IpcToolRegistry`
/// connection through Handshake, `ListTools`, and `CallTool`
#[tokio::test]
async fn test_ipc_list_tools_and_call_tool() {
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
            .expect("Expected Handshake request");
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
            .expect("Expected ListTools request");
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
            .expect("Expected ListRagProfiles request");
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
            .expect("Expected CallTool request");
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
        .expect("Failed to create IpcToolRegistry");

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
        .expect("call_tool failed");
    assert_eq!(result, "2024-01-01 12:00:00");

    // Wait for server to shut down
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("Server timed out")
        .expect("Server panicked");
}
