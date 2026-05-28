use ene_tool_host::ToolRegistry;
use ene_tool_proto::transport::IpcListener;
use ene_tool_proto::{
    IpcRequest, IpcResponse, IPC_PROTOCOL_VERSION, SandboxConfigData, ToolCategory, ToolDefinition,
    read_ipc_request, write_ipc_response,
};
use std::path::PathBuf;
use std::time::Duration;

/// End-to-end test of the IPC protocol
///
/// Sets up a mock IPC server and verifies the full flow from IpcToolRegistry
/// connection through Handshake, ListTools, and CallTool
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
            matches!(req, IpcRequest::Handshake { version: IPC_PROTOCOL_VERSION }),
            "Expected Handshake, got {:?}",
            req
        );
        write_ipc_response(
            &mut stream,
            &IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            },
        )
        .await
        .unwrap();

        // 2. Initialize
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("Expected Initialize request");
        assert!(
            matches!(req, IpcRequest::Initialize { .. }),
            "Expected Initialize, got {:?}",
            req
        );
        write_ipc_response(&mut stream, &IpcResponse::Ack)
            .await
            .unwrap();

        // 3. ListTools (refresh_tools is called inside IpcToolRegistry::new)
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("Expected ListTools request");
        assert!(
            matches!(req, IpcRequest::ListTools),
            "Expected ListTools, got {:?}",
            req
        );
        write_ipc_response(
            &mut stream,
            &IpcResponse::Tools {
                tools: vec![ToolDefinition {
                    name: "get_current_time".to_string(),
                    description: "Get the current date and time.".to_string(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                    category: Some(ToolCategory::Utility),
                    keywords: vec!["time".to_string()],
                }],
            },
        )
        .await
        .unwrap();

        // 3. CallTool
        let req = read_ipc_request(&mut stream)
            .await
            .unwrap()
            .expect("Expected CallTool request");
        match &req {
            IpcRequest::CallTool { name, arguments } => {
                assert_eq!(name, "get_current_time");
                assert_eq!(arguments, "{}");
            }
            _ => panic!("Expected CallTool, got {:?}", req),
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
        undo_db_path: None,
    };

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, sandbox, None)
        .await
        .expect("Failed to create IpcToolRegistry");

    // Verify list_tools
    let tools = registry.list_tools();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "get_current_time");

    // Verify call_tool
    let result = registry
        .call_tool("get_current_time", "{}")
        .await
        .expect("call_tool failed");
    assert_eq!(result, "2024-01-01 12:00:00");

    // Wait for server to shut down
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("Server timed out")
        .expect("Server panicked");
}

/// End-to-end test that launches the actual ene-tool-host binary
#[tokio::test]
async fn test_ipc_with_real_host() {
    let socket_path: PathBuf = {
        #[cfg(unix)]
        {
            let p = PathBuf::from("/tmp/ene-test-real-host.sock");
            ene_tool_proto::transport::cleanup_path(&p);
            p
        }
        #[cfg(windows)]
        {
            PathBuf::from(r"\\.\pipe\ene-test-real-host")
        }
    };

    let bin_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("target")
        .join("debug")
        .join("ene-tool-host");

    let mut host = if bin_path.exists() {
        tokio::process::Command::new(&bin_path)
    } else {
        let mut cmd = tokio::process::Command::new("cargo");
        cmd.args(["run", "--bin", "ene-tool-host", "--"]);
        cmd
    };

    host.env("ENE_TOOL_SOCKET", &socket_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = host.spawn().expect("Failed to start ene-tool-host");

    // Wait for host to start
    tokio::time::sleep(Duration::from_millis(800)).await;

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
        undo_db_path: None,
    };

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, sandbox, None)
        .await
        .expect("Failed to connect to real tool host");

    let tools = registry.list_tools();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"get_current_time"),
        "Expected get_current_time in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"get_system_info"),
        "Expected get_system_info in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"webfetch"),
        "Expected webfetch in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"websearch"),
        "Expected websearch in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"question"),
        "Expected question in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"todo"),
        "Expected todo in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"app"),
        "Expected app in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"browser"),
        "Expected browser in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"filesystem"),
        "Expected filesystem in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"shell"),
        "Expected shell in tools, got: {:?}",
        names
    );
    assert!(
        names.contains(&"undo"),
        "Expected undo in tools, got: {:?}",
        names
    );

    let result = registry
        .call_tool("get_current_time", "{}")
        .await
        .expect("call_tool get_current_time failed");
    assert!(!result.is_empty(), "Result should not be empty");
    println!("Real host get_current_time result: {}", result);

    let result = registry
        .call_tool("get_system_info", "{}")
        .await
        .expect("call_tool get_system_info failed");
    assert!(
        result.contains("OS:"),
        "Result should contain OS info: {}",
        result
    );
    println!("Real host get_system_info result: {}", result);

    let result = registry
        .call_tool(
            "question",
            r#"{"questions": ["What is your name?", "What is your favorite color?"]}"#,
        )
        .await
        .expect("call_tool question failed");
    assert!(
        result.contains("What is your name?"),
        "Result should contain question: {}",
        result
    );
    println!("Real host question result: {}", result);

    let result = registry
        .call_tool(
            "todo",
            r#"{"todos": [{"content": "Test task", "status": "pending", "priority": "high"}]}"#,
        )
        .await
        .expect("call_tool todo failed");
    assert!(
        result.contains("Test task"),
        "Result should contain todo: {}",
        result
    );
    println!("Real host todo result: {}", result);

    let _ = child.kill().await;
}
