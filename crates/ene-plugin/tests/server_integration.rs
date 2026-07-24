//! Plugin server integration tests.
//!
//! These tests verify the full round-trip from a raw IPC client through the
//! wire protocol to a [`Plugin`] trait implementation. A minimal server loop
//! (mirroring [`ene_plugin::run_plugin_server`]) accepts connections on a
//! Unix socket, dispatches requests to the plugin, and writes responses.
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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use ene_plugin::{Plugin, PluginCapabilities};
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, IpcListener, IpcStream,
    PLUGIN_IPC_PROTOCOL_VERSION, PluginIpcRequest, PluginIpcResponse, SandboxConfigData, ToolError,
    ToolName, ToolSpec, cleanup_path, read_plugin_request, read_plugin_response,
    write_plugin_request, write_plugin_response,
};
use tokio::sync::Mutex;

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/ene-plugin-test-{}-{id}-{name}.sock",
        std::process::id()
    ))
}

/// State recorded by the test plugin for assertion.
#[derive(Debug, Default)]
struct TestPluginState {
    sandbox_received: AtomicBool,
    config_received: AtomicBool,
    call_context: Mutex<Option<CallContext>>,
    approved: Mutex<Vec<String>>,
    allowed: Mutex<Vec<(String, String)>>,
    revoked: Mutex<Vec<(String, String)>>,
    cancelled: Mutex<Vec<String>>,
}

/// A plugin that exercises every interactive method.
struct TestPlugin {
    state: Arc<TestPluginState>,
}

#[async_trait]
impl Plugin for TestPlugin {
    fn capabilities(&self) -> PluginCapabilities {
        PluginCapabilities {
            tools: vec![ToolSpec::new(
                ToolName::new("test.echo"),
                "Echoes input.",
                serde_json::json!({}),
            )],
            llm_providers: vec![],
            tts_providers: vec![],
            stt_providers: vec![],
        }
    }

    async fn call_tool(&self, name: &str, args: &str) -> Result<String, ToolError> {
        match name {
            "test.echo" => Ok(args.to_string()),
            "test.permission" => Err(ToolError::PermissionRequired {
                request_id: "req-perm-1".to_string(),
                action: "shell_exec".to_string(),
                target: "rm -rf /".to_string(),
                description: "Dangerous command".to_string(),
            }),
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredOutcome, ToolError> {
        if name == "test.background" {
            Ok(DeferredOutcome::Deferred {
                task_id: "bg-task-1".to_string(),
            })
        } else {
            Ok(DeferredOutcome::Sync(
                self.call_tool(name, arguments).await?,
            ))
        }
    }

    fn poll_deferred(&self, task_id: &str) -> Result<DeferredStatus, ToolError> {
        if task_id == "bg-task-1" {
            Ok(DeferredStatus::Completed {
                result: "background done".to_string(),
            })
        } else {
            Ok(DeferredStatus::Unknown)
        }
    }

    fn cancel_deferred(&self, task_id: &str) -> Result<(), ToolError> {
        let mut cancelled = self.state.cancelled.try_lock().unwrap();
        cancelled.push(task_id.to_string());
        Ok(())
    }

    fn set_call_context(&self, ctx: &CallContext) -> Result<(), ToolError> {
        let mut guard = self.state.call_context.try_lock().unwrap();
        *guard = Some(ctx.clone());
        Ok(())
    }

    fn approve_permission(&self, request_id: &str) -> Result<(), ToolError> {
        let mut approved = self.state.approved.try_lock().unwrap();
        approved.push(request_id.to_string());
        Ok(())
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        let mut allowed = self.state.allowed.try_lock().unwrap();
        allowed.push((action.to_string(), target_pattern.to_string()));
        Ok(())
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) -> Result<(), ToolError> {
        let mut revoked = self.state.revoked.try_lock().unwrap();
        revoked.push((action.to_string(), target_pattern.to_string()));
        Ok(())
    }

    fn set_config(&self, _config: &serde_json::Value) {
        self.state.config_received.store(true, Ordering::SeqCst);
    }

    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {
        self.state.sandbox_received.store(true, Ordering::SeqCst);
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({"type": "object", "properties": {"key": {"type": "string"}}}))
    }
}

/// Dispatches a request to the plugin, mirroring `ene_plugin::server::dispatch`.
async fn dispatch(plugin: &dyn Plugin, req: &PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version,
            sandbox,
            plugin_config,
        } => {
            if *version != PLUGIN_IPC_PROTOCOL_VERSION {
                return PluginIpcResponse::Error {
                    message: format!("version mismatch: got {version}"),
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
            name,
            arguments,
            deferred,
        } => {
            if *deferred {
                match plugin.call_tool_deferred(name, arguments).await {
                    Ok(DeferredOutcome::Sync(result)) => {
                        PluginIpcResponse::CallResult { result: Ok(result) }
                    }
                    Ok(DeferredOutcome::Deferred { task_id }) => {
                        PluginIpcResponse::DeferredAccepted { task_id }
                    }
                    Err(e) => PluginIpcResponse::CallResult { result: Err(e) },
                }
            } else {
                let result = plugin.call_tool(name, arguments).await;
                PluginIpcResponse::CallResult { result }
            }
        }
        PluginIpcRequest::SetCallContext {
            conversation_id,
            turn_id,
        } => {
            let ctx = CallContext {
                conversation_id: conversation_id.clone(),
                turn_id: turn_id.clone(),
            };
            match plugin.set_call_context(&ctx) {
                Ok(()) => PluginIpcResponse::Ack,
                Err(e) => PluginIpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::ApprovePermission { request_id } => {
            match plugin.approve_permission(request_id) {
                Ok(()) => PluginIpcResponse::Ack,
                Err(e) => PluginIpcResponse::Error {
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::AllowPattern {
            action,
            target_pattern,
        } => match plugin.allow_pattern(action, target_pattern) {
            Ok(()) => PluginIpcResponse::Ack,
            Err(e) => PluginIpcResponse::Error {
                message: e.to_string(),
            },
        },
        PluginIpcRequest::RevokePattern {
            action,
            target_pattern,
        } => match plugin.revoke_pattern(action, target_pattern) {
            Ok(()) => PluginIpcResponse::Ack,
            Err(e) => PluginIpcResponse::Error {
                message: e.to_string(),
            },
        },
        PluginIpcRequest::PollDeferred { task_id } => match plugin.poll_deferred(task_id) {
            Ok(status) => PluginIpcResponse::DeferredStatus {
                task_id: task_id.clone(),
                status,
            },
            Err(e) => PluginIpcResponse::Error {
                message: e.to_string(),
            },
        },
        PluginIpcRequest::CancelDeferred { task_id } => match plugin.cancel_deferred(task_id) {
            Ok(()) => PluginIpcResponse::Ack,
            Err(e) => PluginIpcResponse::Error {
                message: e.to_string(),
            },
        },
        PluginIpcRequest::Shutdown => PluginIpcResponse::Ack,
        _ => PluginIpcResponse::Error {
            message: "unsupported request in test dispatch".to_string(),
        },
    }
}

/// Runs a minimal plugin server loop on the given socket path.
async fn run_test_server(socket_path: PathBuf, plugin: Arc<dyn Plugin>) {
    cleanup_path(&socket_path);
    let mut listener = IpcListener::bind(&socket_path).expect("failed to bind test server");

    loop {
        let Ok(mut stream) = listener.accept().await else {
            break;
        };
        let plugin = Arc::clone(&plugin);

        loop {
            let Ok(Some(req)) = read_plugin_request(&mut stream).await else {
                break;
            };
            let resp = dispatch(plugin.as_ref(), &req).await;
            if write_plugin_response(&mut stream, &resp).await.is_err() {
                break;
            }
        }
    }
}

/// Spawns a test plugin server and connects a raw IPC stream to it.
async fn spawn_and_connect(name: &str) -> (IpcStream, Arc<TestPluginState>, PathBuf) {
    let socket_path = test_socket_path(name);
    let state = Arc::new(TestPluginState::default());

    let plugin: Arc<dyn Plugin> = Arc::new(TestPlugin {
        state: Arc::clone(&state),
    });

    let server_path = socket_path.clone();
    tokio::spawn(async move {
        run_test_server(server_path, plugin).await;
    });

    // Wait for the server to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let stream = IpcStream::connect(&socket_path)
        .await
        .expect("should connect to plugin server");

    (stream, state, socket_path)
}

/// Performs the handshake on a raw stream and returns the capabilities.
async fn do_handshake(stream: &mut IpcStream) -> PluginCapabilities {
    write_plugin_request(
        stream,
        &PluginIpcRequest::Handshake {
            version: PLUGIN_IPC_PROTOCOL_VERSION,
            sandbox: SandboxConfigData::default(),
            plugin_config: Some(serde_json::json!({"test_key": "test_value"})),
        },
    )
    .await
    .expect("write handshake");

    let resp = read_plugin_response(stream)
        .await
        .expect("read handshake ack")
        .expect("non-EOF");

    match resp {
        PluginIpcResponse::HandshakeAck {
            version,
            capabilities,
        } => {
            assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
            capabilities
        }
        other => panic!("expected HandshakeAck, got: {other:?}"),
    }
}

/// Sends a request and reads the single response.
async fn round_trip(stream: &mut IpcStream, req: &PluginIpcRequest) -> PluginIpcResponse {
    write_plugin_request(stream, req)
        .await
        .expect("write request");
    read_plugin_response(stream)
        .await
        .expect("read response")
        .expect("non-EOF")
}

// ── Tests ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn server_handshake_advertises_capabilities() {
    let (mut stream, state, socket_path) = spawn_and_connect("hs").await;

    let caps = do_handshake(&mut stream).await;
    assert_eq!(caps.tools.len(), 1);
    assert_eq!(caps.tools[0].name.as_str(), "test.echo");

    // Verify sandbox and config were received during handshake.
    assert!(state.sandbox_received.load(Ordering::SeqCst));
    assert!(state.config_received.load(Ordering::SeqCst));

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_call_tool_echo() {
    let (mut stream, _state, socket_path) = spawn_and_connect("echo").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CallTool {
            name: "test.echo".to_string(),
            arguments: r#"{"data":"value"}"#.to_string(),
            deferred: false,
        },
    )
    .await;

    match resp {
        PluginIpcResponse::CallResult { result } => {
            assert_eq!(result.unwrap(), r#"{"data":"value"}"#);
        }
        other => panic!("expected CallResult, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_call_tool_structured_error() {
    let (mut stream, _state, socket_path) = spawn_and_connect("err").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CallTool {
            name: "test.permission".to_string(),
            arguments: "{}".to_string(),
            deferred: false,
        },
    )
    .await;

    match resp {
        PluginIpcResponse::CallResult { result } => {
            let err = result.expect_err("should be Err");
            match err {
                ToolError::PermissionRequired {
                    request_id, action, ..
                } => {
                    assert_eq!(request_id, "req-perm-1");
                    assert_eq!(action, "shell_exec");
                }
                other => panic!("expected PermissionRequired, got: {other:?}"),
            }
        }
        other => panic!("expected CallResult, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_set_call_context_forwarded() {
    let (mut stream, state, socket_path) = spawn_and_connect("ctx").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::SetCallContext {
            conversation_id: "conv-xyz".to_string(),
            turn_id: "turn-3".to_string(),
        },
    )
    .await;
    assert_eq!(resp, PluginIpcResponse::Ack);

    let ctx = state.call_context.lock().await;
    let ctx = ctx.as_ref().expect("context should be set");
    assert_eq!(ctx.conversation_id, "conv-xyz");
    assert_eq!(ctx.turn_id, "turn-3");

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_approve_permission_forwarded() {
    let (mut stream, state, socket_path) = spawn_and_connect("approve").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::ApprovePermission {
            request_id: "req-abc".to_string(),
        },
    )
    .await;
    assert_eq!(resp, PluginIpcResponse::Ack);

    let approved = state.approved.lock().await;
    assert_eq!(*approved, vec!["req-abc"]);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_allow_and_revoke_pattern_forwarded() {
    let (mut stream, state, socket_path) = spawn_and_connect("patterns").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::AllowPattern {
            action: "net_access".to_string(),
            target_pattern: "*.example.com".to_string(),
        },
    )
    .await;
    assert_eq!(resp, PluginIpcResponse::Ack);

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::RevokePattern {
            action: "net_access".to_string(),
            target_pattern: "*.example.com".to_string(),
        },
    )
    .await;
    assert_eq!(resp, PluginIpcResponse::Ack);

    let allowed = state.allowed.lock().await;
    assert_eq!(
        *allowed,
        vec![("net_access".to_string(), "*.example.com".to_string())]
    );
    let revoked = state.revoked.lock().await;
    assert_eq!(
        *revoked,
        vec![("net_access".to_string(), "*.example.com".to_string())]
    );

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_deferred_call_and_poll() {
    let (mut stream, _state, socket_path) = spawn_and_connect("deferred").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CallTool {
            name: "test.background".to_string(),
            arguments: "{}".to_string(),
            deferred: true,
        },
    )
    .await;
    match resp {
        PluginIpcResponse::DeferredAccepted { task_id } => {
            assert_eq!(task_id, "bg-task-1");
        }
        other => panic!("expected DeferredAccepted, got: {other:?}"),
    }

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::PollDeferred {
            task_id: "bg-task-1".to_string(),
        },
    )
    .await;
    match resp {
        PluginIpcResponse::DeferredStatus { task_id, status } => {
            assert_eq!(task_id, "bg-task-1");
            assert_eq!(
                status,
                DeferredStatus::Completed {
                    result: "background done".to_string()
                }
            );
        }
        other => panic!("expected DeferredStatus, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_deferred_sync_fallback() {
    let (mut stream, _state, socket_path) = spawn_and_connect("def-sync").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CallTool {
            name: "test.echo".to_string(),
            arguments: r#"{"sync":true}"#.to_string(),
            deferred: true,
        },
    )
    .await;
    match resp {
        PluginIpcResponse::CallResult { result } => {
            assert_eq!(result.unwrap(), r#"{"sync":true}"#);
        }
        other => panic!("expected CallResult (sync fallback), got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_cancel_deferred_forwarded() {
    let (mut stream, state, socket_path) = spawn_and_connect("cancel").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CancelDeferred {
            task_id: "bg-task-1".to_string(),
        },
    )
    .await;
    assert_eq!(resp, PluginIpcResponse::Ack);

    let cancelled = state.cancelled.lock().await;
    assert_eq!(*cancelled, vec!["bg-task-1"]);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_ping_pong() {
    let (mut stream, _state, socket_path) = spawn_and_connect("ping").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(&mut stream, &PluginIpcRequest::Ping).await;
    assert_eq!(resp, PluginIpcResponse::Pong);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_config_schema() {
    let (mut stream, _state, socket_path) = spawn_and_connect("schema").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(&mut stream, &PluginIpcRequest::GetConfigSchema).await;
    match resp {
        PluginIpcResponse::ConfigSchema { schema } => {
            let schema = schema.expect("schema should be present");
            assert_eq!(schema["type"], "object");
        }
        other => panic!("expected ConfigSchema, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_list_tools() {
    let (mut stream, _state, socket_path) = spawn_and_connect("list").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(&mut stream, &PluginIpcRequest::ListTools).await;
    match resp {
        PluginIpcResponse::Tools { tools } => {
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name.as_str(), "test.echo");
        }
        other => panic!("expected Tools, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}
