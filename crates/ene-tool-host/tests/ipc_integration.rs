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
            IpcRequest::CallTool {
                name,
                arguments,
                deferred: _,
            } => {
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

/// When the tool binary returns an `Error` response for `CallTool`, the
/// registry surfaces it as `ExecutionFailed`.
#[tokio::test]
async fn call_tool_error_response_is_surfaced() {
    let socket_path = test_socket("call-err");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            while let Ok(Some(req)) = read_ipc_request(&mut stream).await {
                if let IpcRequest::CallTool { .. } = req {
                    write_ipc_response(
                        &mut stream,
                        &IpcResponse::Error {
                            message: "tool exploded".into(),
                        },
                    )
                    .await
                    .unwrap();
                }
            }
        });
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    let err = registry
        .call_tool("test.echo", "{}")
        .await
        .expect_err("tool returns an error");
    let msg = err.to_string();
    assert!(
        msg.contains("tool exploded"),
        "error message should propagate: {msg}"
    );

    server.abort();
}

/// When the tool binary returns a `CallResult` with an `Err` payload, the
/// registry converts it into a `ToolHostError`.
#[tokio::test]
async fn call_tool_call_result_err_is_propagated() {
    let socket_path = test_socket("call-res-err");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            while let Ok(Some(req)) = read_ipc_request(&mut stream).await {
                if let IpcRequest::CallTool { .. } = req {
                    write_ipc_response(
                        &mut stream,
                        &IpcResponse::CallResult {
                            result: Err(ene_tool_proto::ToolError::NotFound {
                                tool_name: "nope".into(),
                            }),
                        },
                    )
                    .await
                    .unwrap();
                }
            }
        });
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    let err = registry
        .call_tool("test.echo", "{}")
        .await
        .expect_err("tool returns CallResult::Err");
    let msg = err.to_string();
    assert!(
        msg.contains("nope") || msg.contains("not found"),
        "error should mention the tool: {msg}"
    );

    server.abort();
}

/// A tool that hangs on `CallTool` triggers the per-call timeout rather
/// than blocking indefinitely.
#[tokio::test]
async fn call_tool_timeout_on_hung_tool() {
    let socket_path = test_socket("call-timeout");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            // Read requests but never respond.
            while let Ok(Some(_)) = read_ipc_request(&mut stream).await {}
        });
    });

    // 200ms timeout for fast test execution.
    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 200)
        .await
        .expect("socket server is reachable");

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        registry.call_tool("test.echo", "{}"),
    )
    .await;
    match result {
        Ok(Err(e)) => {
            let msg = e.to_string();
            assert!(
                msg.contains("timed out") || msg.contains("timeout"),
                "expected timeout error: {msg}"
            );
        }
        Ok(Ok(_)) => panic!("hung tool should not produce a result"),
        Err(elapsed) => panic!("call_tool itself hung past the test timeout: {elapsed}"),
    }

    server.abort();
}

/// After a disconnect, `send_with_reconnect` re-establishes the connection
/// and retries the request.
#[tokio::test]
async fn reconnect_after_disconnect() {
    let socket_path = test_socket("reconnect");

    // Single listener that accepts two connections: the initial one and
    // the reconnection after the first is dropped.
    let mut listener = IpcListener::bind(&socket_path).unwrap();
    let server = tokio::spawn(async move {
        // ── First connection: handshake + refresh, then drop ──
        let mut stream = listener.accept().await.unwrap();
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // Handshake
        write_ipc_response(
            &mut stream,
            &IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            },
        )
        .await
        .unwrap();
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // ListTools
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
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // ListRagProfiles
        write_ipc_response(
            &mut stream,
            &IpcResponse::RagProfiles {
                profiles: Vec::new(),
            },
        )
        .await
        .unwrap();
        drop(stream); // Simulate disconnect.

        // ── Second connection: re-handshake + refresh + call ──
        let mut stream = listener.accept().await.unwrap();
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // Handshake
        write_ipc_response(
            &mut stream,
            &IpcResponse::HandshakeAck {
                version: IPC_PROTOCOL_VERSION,
            },
        )
        .await
        .unwrap();
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // ListTools
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
        let _ = read_ipc_request(&mut stream).await.unwrap().unwrap(); // ListRagProfiles
        write_ipc_response(
            &mut stream,
            &IpcResponse::RagProfiles {
                profiles: Vec::new(),
            },
        )
        .await
        .unwrap();
        let req = read_ipc_request(&mut stream).await.unwrap().unwrap(); // CallTool
        assert!(matches!(req, IpcRequest::CallTool { .. }));
        write_ipc_response(
            &mut stream,
            &IpcResponse::CallResult {
                result: Ok("reconnected!".into()),
            },
        )
        .await
        .unwrap();
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("first connection succeeds");

    // Give the server task time to drop the first stream and reach the
    // second `accept()` call.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The first do_request will fail (connection dropped), triggering
    // ensure_connected → reconnect → retry.
    let result = registry
        .call_tool("test.echo", "{}")
        .await
        .expect("reconnect should succeed");
    assert_eq!(result, "reconnected!");

    server.abort();
}

/// When `list_rag_profiles` has an empty cache (e.g. profiles were never
/// refreshed), it falls back to synthesizing profiles from `list_tools`.
#[tokio::test]
async fn list_rag_profiles_fallback_synthesizes_from_tools() {
    let socket_path = test_socket("rag-fallback");
    let server = spawn_mock_server(&socket_path, |_stream| {
        // No further interaction needed.
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    // The mock server returned empty profiles, so the fallback path
    // synthesizes from the tool list.
    let profiles = registry.list_rag_profiles();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles.first().map(|p| p.name.as_str()), Some("test.echo"));

    server.abort();
}

/// `get_config_schema` returns `None` when the tool responds with an error.
#[tokio::test]
async fn config_schema_returns_none_on_error() {
    let socket_path = test_socket("schema-err");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            while let Ok(Some(req)) = read_ipc_request(&mut stream).await {
                if let IpcRequest::GetConfigSchema = req {
                    write_ipc_response(
                        &mut stream,
                        &IpcResponse::Error {
                            message: "no schema".into(),
                        },
                    )
                    .await
                    .unwrap();
                }
            }
        });
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    let schema = registry.get_config_schema().await;
    assert!(schema.is_none());

    server.abort();
}

/// `get_config_schema` returns the schema when the tool responds correctly.
#[tokio::test]
async fn config_schema_returns_value_on_success() {
    let socket_path = test_socket("schema-ok");
    let server = spawn_mock_server(&socket_path, |mut stream| {
        tokio::spawn(async move {
            while let Ok(Some(req)) = read_ipc_request(&mut stream).await {
                if let IpcRequest::GetConfigSchema = req {
                    write_ipc_response(
                        &mut stream,
                        &IpcResponse::ConfigSchema {
                            schema: Some(serde_json::json!({
                                "type": "object",
                                "properties": {"level": {"type": "integer"}}
                            })),
                        },
                    )
                    .await
                    .unwrap();
                }
            }
        });
    });

    let registry = ene_tool_host::IpcToolRegistry::new(socket_path, test_sandbox(), None, 60_000)
        .await
        .expect("socket server is reachable");

    let schema = registry.get_config_schema().await;
    assert!(schema.is_some());
    let s = schema.unwrap();
    assert!(s.get("properties").and_then(|p| p.get("level")).is_some());

    server.abort();
}
