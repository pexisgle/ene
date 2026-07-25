//! Host ↔ Plugin IPC round-trip integration tests.
//!
//! These tests spin up a mock plugin server on a Unix domain socket and
//! exercise [`IpcPluginConnection`] against it, verifying the full
//! request/response cycle over the v3 wire protocol.
#![allow(
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests use unwrap/expect/panic for assertions"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use ene_plugin_host::{IpcPluginConnection, PluginHostError};
use ene_plugin_proto::{
    CallContext, DeferredStatus, IpcListener, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities,
    PluginIpcRequest, PluginIpcResponse, SandboxConfigData, ToolError, ToolName, ToolResult,
    ToolSpec, VersionRange, cleanup_path, read_plugin_request, write_plugin_response,
};
use tokio::sync::Mutex;

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Returns a unique socket path for a test.
fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/ene-host-test-{}-{id}-{name}.sock",
        std::process::id()
    ))
}

/// Recorded state from the mock plugin server, shared with test assertions.
#[derive(Debug, Default)]
struct MockState {
    call_context: Option<CallContext>,
    approved: Vec<String>,
    allowed: Vec<(String, String)>,
    revoked: Vec<(String, String)>,
    cancelled: Vec<String>,
}

/// Runs a mock plugin server that handles all v3 request types.
///
/// Accepts connections in a loop until the listener is dropped or the
/// socket is cleaned up. Each connection is handled sequentially.
async fn run_mock_server(socket_path: PathBuf, state: Arc<Mutex<MockState>>) {
    cleanup_path(&socket_path);
    let mut listener = IpcListener::bind(&socket_path).expect("failed to bind mock server");

    loop {
        let Ok(mut stream) = listener.accept().await else {
            break;
        };
        let state = Arc::clone(&state);

        loop {
            let Ok(Some(req)) = read_plugin_request(&mut stream).await else {
                break;
            };

            let resp = dispatch_mock(&state, req).await;
            if write_plugin_response(&mut stream, &resp).await.is_err() {
                break;
            }
        }
    }
}

/// Dispatches a single request to mock behavior.
async fn dispatch_mock(state: &Mutex<MockState>, req: PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            sandbox: _,
            plugin_config: _,
        } => {
            let our_range = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            };
            if host_range.max < our_range.min || host_range.min > our_range.max {
                return PluginIpcResponse::Error {
                    request_id: String::new(),
                    message: format!(
                        "version mismatch: host supports {}-{}, mock supports {}-{}",
                        host_range.min, host_range.max, our_range.min, our_range.max
                    ),
                };
            }
            let negotiated = host_range.min.max(our_range.min);
            PluginIpcResponse::HandshakeAck {
                version: negotiated,
                capabilities: PluginCapabilities {
                    tools: 1,
                    llm_providers: vec![],
                    tts_providers: vec![],
                    stt_providers: vec![],
                },
            }
        }
        PluginIpcRequest::ListTools { request_id } => PluginIpcResponse::Tools {
            request_id,
            tools: vec![ToolSpec::new(
                ToolName::new("mock.echo"),
                "Echoes arguments back.",
                serde_json::json!({}),
            )],
        },
        PluginIpcRequest::Ping => PluginIpcResponse::Pong,
        PluginIpcRequest::CallTool {
            request_id,
            name,
            arguments,
            deferred,
            context,
        } => {
            // When context is provided, record it on the mock state so tests
            // can verify the per-call context is forwarded correctly.
            if let Some(ctx) = context {
                let mut s = state.lock().await;
                s.call_context = Some(ctx);
            }
            if deferred && name == "mock.deferred" {
                return PluginIpcResponse::DeferredAccepted {
                    request_id,
                    task_id: "task-42".to_string(),
                };
            }
            match name.as_str() {
                "mock.echo" => PluginIpcResponse::CallResult {
                    request_id,
                    result: Ok(ToolResult::from_string(arguments)),
                },
                "mock.permission" => PluginIpcResponse::CallResult {
                    request_id,
                    result: Err(ToolError::PermissionRequired {
                        request_id: "perm-1".to_string(),
                        action: "filesystem_write".to_string(),
                        target: "/etc/passwd".to_string(),
                        description: "Write access to /etc/passwd".to_string(),
                    }),
                },
                "mock.user_input" => PluginIpcResponse::CallResult {
                    request_id,
                    result: Err(ToolError::UserInputRequired {
                        request_id: "input-1".to_string(),
                        prompt: ene_plugin_proto::UserInputPrompt::new(vec![
                            ene_plugin_proto::QuestionItem {
                                question: "Continue?".to_string(),
                                options: vec!["Yes".to_string(), "No".to_string()],
                                allow_free_text: false,
                            },
                        ])
                        .expect("non-empty prompt"),
                    }),
                },
                _ => PluginIpcResponse::CallResult {
                    request_id,
                    result: Err(ToolError::NotFound { tool_name: name }),
                },
            }
        }
        PluginIpcRequest::SetCallContext {
            request_id,
            conversation_id: _,
            turn_id: _,
        } => {
            tracing::warn!(
                component = "IpcIntegrationMock",
                "SetCallContext is deprecated and handled as a no-op in the mock server"
            );
            PluginIpcResponse::Ack { request_id }
        }
        PluginIpcRequest::ApprovePermission {
            request_id,
            permission_request_id,
        } => {
            let mut s = state.lock().await;
            s.approved.push(permission_request_id);
            PluginIpcResponse::Ack { request_id }
        }
        PluginIpcRequest::AllowPattern {
            request_id,
            action,
            target_pattern,
        } => {
            let mut s = state.lock().await;
            s.allowed.push((action, target_pattern));
            PluginIpcResponse::Ack { request_id }
        }
        PluginIpcRequest::RevokePattern {
            request_id,
            action,
            target_pattern,
        } => {
            let mut s = state.lock().await;
            s.revoked.push((action, target_pattern));
            PluginIpcResponse::Ack { request_id }
        }
        PluginIpcRequest::PollDeferred {
            request_id,
            task_id,
        } => {
            if task_id == "task-42" {
                PluginIpcResponse::DeferredStatus {
                    request_id,
                    task_id,
                    status: DeferredStatus::Completed {
                        result: "deferred result".to_string(),
                    },
                }
            } else {
                PluginIpcResponse::DeferredStatus {
                    request_id,
                    task_id,
                    status: DeferredStatus::Unknown,
                }
            }
        }
        PluginIpcRequest::CancelDeferred {
            request_id,
            task_id,
        } => {
            let mut s = state.lock().await;
            s.cancelled.push(task_id);
            PluginIpcResponse::Ack { request_id }
        }
        PluginIpcRequest::Shutdown => PluginIpcResponse::Ack {
            request_id: String::new(),
        },
        _ => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "unsupported request in mock".to_string(),
        },
    }
}

/// Spawns a mock server and connects an [`IpcPluginConnection`] to it.
///
/// Returns the connection, the shared mock state, and the socket path
/// (for cleanup).
async fn spawn_and_connect(name: &str) -> (IpcPluginConnection, Arc<Mutex<MockState>>, PathBuf) {
    let socket_path = test_socket_path(name);
    let state = Arc::new(Mutex::new(MockState::default()));

    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        Some(serde_json::json!({"test": true})),
    )
    .await
    .expect("handshake should succeed");

    (conn, state, socket_path)
}

// ── Test: Handshake ──────────────────────────────────────────────────────

#[tokio::test]
async fn handshake_succeeds_and_returns_capabilities() {
    let (mut conn, _state, socket_path) = spawn_and_connect("handshake").await;

    let caps = conn.capabilities();
    assert_eq!(caps.tools, 1);
    assert!(caps.llm_providers.is_empty());

    // Verify ListTools returns the actual spec.
    let tools = conn.list_tools().await.expect("list_tools should succeed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_str(), "mock.echo");

    cleanup_path(&socket_path);
}

// ── Test: call_tool round-trip ───────────────────────────────────────────

#[tokio::test]
async fn call_tool_round_trip() {
    let (mut conn, _state, socket_path) = spawn_and_connect("call-tool").await;

    let result = conn
        .call_tool("mock.echo", r#"{"msg":"hello"}"#, None)
        .await
        .expect("call_tool should succeed");
    assert_eq!(result.text_for_llm(), r#"{"msg":"hello"}"#);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn call_tool_not_found() {
    let (mut conn, _state, socket_path) = spawn_and_connect("call-tool-nf").await;

    let err = conn
        .call_tool("nonexistent.tool", "{}", None)
        .await
        .expect_err("should fail for unknown tool");

    match err {
        PluginHostError::Protocol(ToolError::NotFound { tool_name }) => {
            assert_eq!(tool_name, "nonexistent.tool");
        }
        other => panic!("expected Protocol(NotFound), got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

// ── Test: Structured error propagation ───────────────────────────────────

#[tokio::test]
async fn call_tool_permission_required_preserves_structure() {
    let (mut conn, _state, socket_path) = spawn_and_connect("perm-required").await;

    let err = conn
        .call_tool("mock.permission", "{}", None)
        .await
        .expect_err("should return PermissionRequired");

    match err {
        PluginHostError::Protocol(ToolError::PermissionRequired {
            request_id,
            action,
            target,
            description,
        }) => {
            assert_eq!(request_id, "perm-1");
            assert_eq!(action, "filesystem_write");
            assert_eq!(target, "/etc/passwd");
            assert_eq!(description, "Write access to /etc/passwd");
        }
        other => panic!("expected Protocol(PermissionRequired), got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn call_tool_user_input_required_preserves_structure() {
    let (mut conn, _state, socket_path) = spawn_and_connect("user-input").await;

    let err = conn
        .call_tool("mock.user_input", "{}", None)
        .await
        .expect_err("should return UserInputRequired");

    match err {
        PluginHostError::Protocol(ToolError::UserInputRequired { request_id, prompt }) => {
            assert_eq!(request_id, "input-1");
            assert_eq!(prompt.items.len(), 1);
            assert_eq!(prompt.items[0].question, "Continue?");
            assert_eq!(prompt.items[0].options, vec!["Yes", "No"]);
        }
        other => panic!("expected Protocol(UserInputRequired), got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

// ── Test: per-call context via call_tool ─────────────────────────────────

#[tokio::test]
async fn call_tool_forwards_context() {
    let (mut conn, state, socket_path) = spawn_and_connect("call-ctx").await;

    let ctx = CallContext {
        conversation_id: "conv-abc".to_string(),
        turn_id: "turn-7".to_string(),
    };
    let result = conn
        .call_tool("mock.echo", r#"{"x":1}"#, Some(ctx))
        .await
        .expect("call_tool should succeed");
    assert_eq!(result.text_for_llm(), r#"{"x":1}"#);

    let s = state.lock().await;
    let recorded = s.call_context.as_ref().expect("context should be recorded");
    assert_eq!(recorded.conversation_id, "conv-abc");
    assert_eq!(recorded.turn_id, "turn-7");

    cleanup_path(&socket_path);
}

// ── Test: approve_permission ─────────────────────────────────────────────

#[tokio::test]
async fn approve_permission_reaches_plugin() {
    let (mut conn, state, socket_path) = spawn_and_connect("approve").await;

    conn.approve_permission("perm-99")
        .await
        .expect("approve_permission should succeed");

    let s = state.lock().await;
    assert_eq!(s.approved, vec!["perm-99"]);

    cleanup_path(&socket_path);
}

// ── Test: allow_pattern / revoke_pattern ─────────────────────────────────

#[tokio::test]
async fn allow_and_revoke_pattern_reach_plugin() {
    let (mut conn, state, socket_path) = spawn_and_connect("patterns").await;

    conn.allow_pattern("filesystem_write", "/tmp/**")
        .await
        .expect("allow_pattern should succeed");
    conn.revoke_pattern("filesystem_write", "/tmp/**")
        .await
        .expect("revoke_pattern should succeed");

    let s = state.lock().await;
    assert_eq!(
        s.allowed,
        vec![("filesystem_write".to_string(), "/tmp/**".to_string())]
    );
    assert_eq!(
        s.revoked,
        vec![("filesystem_write".to_string(), "/tmp/**".to_string())]
    );

    cleanup_path(&socket_path);
}

// ── Test: Deferred execution ─────────────────────────────────────────────

#[tokio::test]
async fn deferred_call_returns_accepted_then_poll_completes() {
    let (mut conn, _state, socket_path) = spawn_and_connect("deferred").await;

    let outcome = conn
        .call_tool_deferred("mock.deferred", "{}", None)
        .await
        .expect("call_tool_deferred should succeed");

    match outcome {
        ene_plugin_proto::DeferredOutcome::Deferred { task_id } => {
            assert_eq!(task_id, "task-42");
        }
        other => panic!("expected Deferred, got: {other:?}"),
    }

    let status = conn
        .poll_deferred("task-42")
        .await
        .expect("poll_deferred should succeed");
    assert_eq!(
        status,
        DeferredStatus::Completed {
            result: "deferred result".to_string()
        }
    );

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn deferred_call_sync_fallback() {
    let (mut conn, _state, socket_path) = spawn_and_connect("deferred-sync").await;

    // mock.echo does not support deferred, so the server falls back to sync.
    let outcome = conn
        .call_tool_deferred("mock.echo", r#"{"x":1}"#, None)
        .await
        .expect("call_tool_deferred should succeed with sync fallback");

    match outcome {
        ene_plugin_proto::DeferredOutcome::Sync(result) => {
            assert_eq!(result.text_for_llm(), r#"{"x":1}"#);
        }
        other => panic!("expected Sync, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

// ── Test: cancel_deferred ────────────────────────────────────────────────

#[tokio::test]
async fn cancel_deferred_reaches_plugin() {
    let (mut conn, state, socket_path) = spawn_and_connect("cancel").await;

    conn.cancel_deferred("task-99")
        .await
        .expect("cancel_deferred should succeed");

    let s = state.lock().await;
    assert_eq!(s.cancelled, vec!["task-99"]);

    cleanup_path(&socket_path);
}

// ── Test: Ping ───────────────────────────────────────────────────────────

#[tokio::test]
async fn ping_returns_pong() {
    let (mut conn, _state, socket_path) = spawn_and_connect("ping").await;

    conn.ping().await.expect("ping should succeed");

    cleanup_path(&socket_path);
}

// ── Test: Reconnection ───────────────────────────────────────────────────

#[tokio::test]
async fn transparent_reconnection_after_transport_failure() {
    let socket_path = test_socket_path("reconnect");
    let state = Arc::new(Mutex::new(MockState::default()));

    // Phase 1: start server, connect, make a successful call.
    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    let server_handle = tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut conn = IpcPluginConnection::connect(&socket_path, SandboxConfigData::default(), None)
        .await
        .expect("initial handshake should succeed");

    let result = conn
        .call_tool("mock.echo", "phase1", None)
        .await
        .expect("first call should succeed");
    assert_eq!(result.text_for_llm(), "phase1");

    // Phase 2: kill the server to simulate transport failure.
    server_handle.abort();
    cleanup_path(&socket_path);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Phase 3: restart the server on the same socket path.
    let server_path2 = socket_path.clone();
    let server_state2 = Arc::clone(&state);
    tokio::spawn(async move {
        run_mock_server(server_path2, server_state2).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Phase 4: the next call should transparently reconnect and succeed.
    let result = conn
        .call_tool("mock.echo", "phase2", None)
        .await
        .expect("call after reconnect should succeed");
    assert_eq!(result.text_for_llm(), "phase2");

    cleanup_path(&socket_path);
}
