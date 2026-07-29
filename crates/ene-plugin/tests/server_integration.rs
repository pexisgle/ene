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
    clippy::manual_let_else,
    reason = "integration tests use unwrap/expect/panic for assertions"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use ene_plugin::{PluginDispatch, ToolPlugin, ToolPluginCapabilities};
use ene_plugin_proto::PluginCapabilities;
use ene_plugin_proto::{
    CallContext, DeferredOutcome, DeferredStatus, IpcListener, IpcStream,
    PLUGIN_IPC_PROTOCOL_VERSION, PluginIpcRequest, PluginIpcResponse, SandboxConfigData, ToolError,
    ToolName, ToolResult, ToolSpec, VersionRange, cleanup_path, read_plugin_request,
    read_plugin_response, write_plugin_request, write_plugin_response,
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
impl ToolPlugin for TestPlugin {
    fn tool_capabilities(&self) -> ToolPluginCapabilities {
        ToolPluginCapabilities { tool_count: 1 }
    }

    fn list_tool_specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec::new(
            ToolName::new("test.echo"),
            "Echoes input.",
            serde_json::json!({}),
        )]
    }

    async fn call_tool(
        &self,
        name: &str,
        args: &str,
        context: Option<&CallContext>,
    ) -> Result<ToolResult, ToolError> {
        if let Some(ctx) = context {
            let mut guard = self.state.call_context.lock().await;
            *guard = Some(ctx.clone());
        }
        match name {
            "test.echo" => Ok(ToolResult::text(args.to_string())),
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
        context: Option<&CallContext>,
    ) -> Result<DeferredOutcome, ToolError> {
        if name == "test.background" {
            Ok(DeferredOutcome::Deferred {
                task_id: "bg-task-1".to_string(),
            })
        } else {
            let result = self.call_tool(name, arguments, context).await?;
            Ok(DeferredOutcome::Sync(result))
        }
    }

    fn poll_deferred(&self, task_id: &str) -> Result<DeferredStatus, ToolError> {
        if task_id == "bg-task-1" {
            Ok(DeferredStatus::Completed {
                result: ToolResult::text("background done"),
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

/// Dispatches a request to the plugin, mirroring `ene_plugin::server::dispatch_request`.
async fn dispatch_fn(dispatch: &PluginDispatch, req: &PluginIpcRequest) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            sandbox,
            plugin_config,
        } => {
            let our_range = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            };
            if host_range.max < our_range.min || host_range.min > our_range.max {
                return PluginIpcResponse::Error {
                    request_id: String::new(),
                    message: format!(
                        "version mismatch: host supports {}-{}, plugin supports {}-{}",
                        host_range.min, host_range.max, our_range.min, our_range.max
                    ),
                };
            }
            let negotiated = host_range.max.min(our_range.max);
            if let Some(tool) = &dispatch.tool {
                tool.set_sandbox(sandbox);
                if let Some(config) = plugin_config {
                    tool.set_config(config);
                }
            }
            PluginIpcResponse::HandshakeAck {
                version: negotiated,
                capabilities: ene_plugin::PluginCapabilities {
                    tools: dispatch
                        .tool
                        .as_ref()
                        .map_or(0, |t| t.tool_capabilities().tool_count),
                    ..Default::default()
                },
            }
        }
        PluginIpcRequest::Ping { request_id } => PluginIpcResponse::Pong {
            request_id: request_id.clone(),
        },
        PluginIpcRequest::GetConfigSchema { request_id } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::ConfigSchema {
                        request_id: request_id.clone(),
                        schema: None,
                    };
                }
            };
            PluginIpcResponse::ConfigSchema {
                request_id: request_id.clone(),
                schema: tool.config_schema(),
            }
        }
        PluginIpcRequest::ListTools { request_id } => PluginIpcResponse::Tools {
            request_id: request_id.clone(),
            tools: dispatch
                .tool
                .as_ref()
                .map_or(Vec::new(), |t| t.list_tool_specs()),
        },
        PluginIpcRequest::CallTool {
            request_id,
            name,
            arguments,
            deferred,
            context,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            let ctx_ref = context.as_ref();
            if *deferred {
                match tool.call_tool_deferred(name, arguments, ctx_ref).await {
                    Ok(DeferredOutcome::Sync(result)) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Ok(result),
                    },
                    Ok(DeferredOutcome::Deferred { task_id }) => {
                        PluginIpcResponse::DeferredAccepted {
                            request_id: request_id.clone(),
                            task_id,
                        }
                    }
                    Err(e) => PluginIpcResponse::CallResult {
                        request_id: request_id.clone(),
                        result: Err(e),
                    },
                }
            } else {
                let result = tool.call_tool(name, arguments, ctx_ref).await;
                PluginIpcResponse::CallResult {
                    request_id: request_id.clone(),
                    result,
                }
            }
        }
        PluginIpcRequest::SetCallContext {
            request_id,
            conversation_id: _,
            turn_id: _,
        } => PluginIpcResponse::Ack {
            request_id: request_id.clone(),
        },
        PluginIpcRequest::ApprovePermission {
            request_id,
            permission_request_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            match tool.approve_permission(permission_request_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::AllowPattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            match tool.allow_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::RevokePattern {
            request_id,
            action,
            target_pattern,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            match tool.revoke_pattern(action, target_pattern) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::PollDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            match tool.poll_deferred(task_id) {
                Ok(status) => PluginIpcResponse::DeferredStatus {
                    request_id: request_id.clone(),
                    task_id: task_id.clone(),
                    status,
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::CancelDeferred {
            request_id,
            task_id,
        } => {
            let tool = match &dispatch.tool {
                Some(t) => t,
                None => {
                    return PluginIpcResponse::Error {
                        request_id: request_id.clone(),
                        message: "no tool plugin".to_string(),
                    };
                }
            };
            match tool.cancel_deferred(task_id) {
                Ok(()) => PluginIpcResponse::Ack {
                    request_id: request_id.clone(),
                },
                Err(e) => PluginIpcResponse::Error {
                    request_id: request_id.clone(),
                    message: e.to_string(),
                },
            }
        }
        PluginIpcRequest::Shutdown => PluginIpcResponse::Ack {
            request_id: String::new(),
        },
        _ => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "unsupported request in test dispatch".to_string(),
        },
    }
}

/// Runs a minimal plugin server loop on the given socket path.
async fn run_test_server(socket_path: PathBuf, dispatch: Arc<PluginDispatch>) {
    cleanup_path(&socket_path);
    let mut listener = IpcListener::bind(&socket_path).expect("failed to bind test server");

    loop {
        let Ok(mut stream) = listener.accept().await else {
            break;
        };
        let dispatch_clone = Arc::clone(&dispatch);

        loop {
            let Ok(Some(req)) = read_plugin_request(&mut stream).await else {
                break;
            };
            let resp = dispatch_fn(dispatch_clone.as_ref(), &req).await;
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

    let dispatch = PluginDispatch::new(
        Some(Arc::new(TestPlugin {
            state: Arc::clone(&state),
        })),
        None,
        None,
        None,
        None,
    );

    let server_path = socket_path.clone();
    tokio::spawn(async move {
        run_test_server(server_path, Arc::new(dispatch)).await;
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
            version: VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            },
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
    assert_eq!(caps.tools, 1);

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
            request_id: "req-1".to_string(),
            name: "test.echo".to_string(),
            arguments: r#"{"data":"value"}"#.to_string(),
            deferred: false,
            context: None,
        },
    )
    .await;

    match resp {
        PluginIpcResponse::CallResult { request_id, result } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(result.unwrap().text_for_llm(), r#"{"data":"value"}"#);
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
            request_id: "req-1".to_string(),
            name: "test.permission".to_string(),
            arguments: "{}".to_string(),
            deferred: false,
            context: None,
        },
    )
    .await;

    match resp {
        PluginIpcResponse::CallResult { result, .. } => {
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
async fn server_set_call_context_deprecated_noop() {
    let (mut stream, state, socket_path) = spawn_and_connect("ctx").await;
    do_handshake(&mut stream).await;

    // SetCallContext is deprecated — it returns Ack but does NOT
    // forward the context to the plugin. Per-call context is now
    // passed directly via the `CallTool` request.
    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::SetCallContext {
            request_id: "req-1".to_string(),
            conversation_id: "conv-xyz".to_string(),
            turn_id: "turn-3".to_string(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Ack {
            request_id: "req-1".to_string()
        }
    );

    let ctx = state.call_context.lock().await;
    assert!(ctx.is_none(), "SetCallContext must be a no-op");

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_call_tool_receives_per_call_context() {
    let (mut stream, state, socket_path) = spawn_and_connect("per-call-ctx").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::CallTool {
            request_id: "req-1".to_string(),
            name: "test.echo".to_string(),
            arguments: r#"{"hello":"world"}"#.to_string(),
            deferred: false,
            context: Some(CallContext {
                conversation_id: "conv-abc".to_string(),
                turn_id: "turn-7".to_string(),
            }),
        },
    )
    .await;

    match resp {
        PluginIpcResponse::CallResult { request_id, result } => {
            assert_eq!(request_id, "req-1");
            assert!(result.is_ok());
        }
        other => panic!("expected CallResult, got: {other:?}"),
    }

    let ctx = state.call_context.lock().await;
    let ctx = ctx.as_ref().expect("context should be set via CallTool");
    assert_eq!(ctx.conversation_id, "conv-abc");
    assert_eq!(ctx.turn_id, "turn-7");

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_approve_permission_forwarded() {
    let (mut stream, state, socket_path) = spawn_and_connect("approve").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::ApprovePermission {
            request_id: "req-1".to_string(),
            permission_request_id: "req-abc".to_string(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Ack {
            request_id: "req-1".to_string()
        }
    );

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
            request_id: "req-1".to_string(),
            action: "net_access".to_string(),
            target_pattern: "*.example.com".to_string(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Ack {
            request_id: "req-1".to_string()
        }
    );

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::RevokePattern {
            request_id: "req-2".to_string(),
            action: "net_access".to_string(),
            target_pattern: "*.example.com".to_string(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Ack {
            request_id: "req-2".to_string()
        }
    );

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
            request_id: "req-1".to_string(),
            name: "test.background".to_string(),
            arguments: "{}".to_string(),
            deferred: true,
            context: None,
        },
    )
    .await;
    match resp {
        PluginIpcResponse::DeferredAccepted {
            request_id,
            task_id,
        } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(task_id, "bg-task-1");
        }
        other => panic!("expected DeferredAccepted, got: {other:?}"),
    }

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::PollDeferred {
            request_id: "req-2".to_string(),
            task_id: "bg-task-1".to_string(),
        },
    )
    .await;
    match resp {
        PluginIpcResponse::DeferredStatus {
            request_id,
            task_id,
            status,
        } => {
            assert_eq!(request_id, "req-2");
            assert_eq!(task_id, "bg-task-1");
            assert_eq!(
                status,
                DeferredStatus::Completed {
                    result: ToolResult::text("background done")
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
            request_id: "req-1".to_string(),
            name: "test.echo".to_string(),
            arguments: r#"{"sync":true}"#.to_string(),
            deferred: true,
            context: None,
        },
    )
    .await;
    match resp {
        PluginIpcResponse::CallResult { request_id, result } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(result.unwrap().text_for_llm(), r#"{"sync":true}"#);
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
            request_id: "req-1".to_string(),
            task_id: "bg-task-1".to_string(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Ack {
            request_id: "req-1".to_string()
        }
    );

    let cancelled = state.cancelled.lock().await;
    assert_eq!(*cancelled, vec!["bg-task-1"]);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_ping_pong() {
    let (mut stream, _state, socket_path) = spawn_and_connect("ping").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::Ping {
            request_id: "ping-1".into(),
        },
    )
    .await;
    assert_eq!(
        resp,
        PluginIpcResponse::Pong {
            request_id: "ping-1".into()
        }
    );

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn server_config_schema() {
    let (mut stream, _state, socket_path) = spawn_and_connect("schema").await;
    do_handshake(&mut stream).await;

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::GetConfigSchema {
            request_id: "req-1".to_string(),
        },
    )
    .await;
    match resp {
        PluginIpcResponse::ConfigSchema { request_id, schema } => {
            assert_eq!(request_id, "req-1");
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

    let resp = round_trip(
        &mut stream,
        &PluginIpcRequest::ListTools {
            request_id: "req-1".to_string(),
        },
    )
    .await;
    match resp {
        PluginIpcResponse::Tools { request_id, tools } => {
            assert_eq!(request_id, "req-1");
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0].name.as_str(), "test.echo");
        }
        other => panic!("expected Tools, got: {other:?}"),
    }

    cleanup_path(&socket_path);
}
