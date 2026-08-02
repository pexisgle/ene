//! Host ↔ Plugin IPC round-trip integration tests.
//!
//! These tests spin up a mock plugin server on a Unix domain socket and
//! exercise [`IpcPluginConnection`] against it, verifying the full
//! request/response cycle over the v3 wire protocol.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use expect/panic for assertions"
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use ene_plugin_host::{IpcPluginConnection, PluginHostError};
use ene_plugin_proto::{
    CallContext, DeferredStatus, IpcListener, PLUGIN_IPC_MIN_SUPPORTED_VERSION,
    PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest, PluginIpcResponse,
    SandboxConfigData, ToolError, ToolName, ToolResult, ToolSpec, VersionRange, cleanup_path,
    read_plugin_request, write_plugin_response,
};
use tokio::sync::{Mutex, Notify};

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Handshake timeout used by the integration tests. Generous enough that a
/// well-behaved mock server always responds in time, yet bounded so a
/// non-responding peer fails the test promptly instead of hanging CI.
const TEST_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Concurrency bound passed to [`IpcPluginConnection::connect`] in tests.
/// Generous enough that ordinary sequential tests never hit it; the dedicated
/// concurrency-bound test uses its own small value.
const TEST_MAX_CONCURRENT: usize = 8;

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
    /// Set once the mock has emitted a `DeferredCompleted` push, so the push
    /// is only emitted a single time (Cr-5 test).
    pushed: bool,
    /// The config blob the host delivered during the handshake, if any.
    plugin_config: Option<serde_json::Value>,
    /// The per-profile blobs the host delivered during the handshake, if any.
    plugin_profiles: Option<serde_json::Value>,
}

/// Runs a mock plugin server that handles all v3 request types.
///
/// Accepts connections in a loop until the listener is dropped or the
/// socket is cleaned up. Each connection is handled sequentially.
///
/// `plugin_range` is the version range the mock declares during the
/// handshake, letting tests simulate a plugin binary built against an older
/// (or unsupported) protocol version.
async fn run_mock_server(socket_path: PathBuf, state: Arc<Mutex<MockState>>) {
    run_mock_server_with_version(
        socket_path,
        state,
        VersionRange {
            min: PLUGIN_IPC_PROTOCOL_VERSION,
            max: PLUGIN_IPC_PROTOCOL_VERSION,
        },
    )
    .await;
}

/// Like [`run_mock_server`], but lets the caller control the plugin's
/// advertised version range for handshake negotiation tests.
async fn run_mock_server_with_version(
    socket_path: PathBuf,
    state: Arc<Mutex<MockState>>,
    plugin_range: VersionRange,
) {
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

            // Cr-5: a `mock.push` tool call triggers a `DeferredCompleted`
            // push frame ahead of the normal response, exercising the host's
            // single-reader push routing (the push must be cached, not
            // swallowed by request/response correlation).
            let emit_push =
                matches!(&req, PluginIpcRequest::CallTool { name, .. } if name == "mock.push");

            let resp = dispatch_mock(&state, req, plugin_range).await;

            if emit_push {
                let mut s = state.lock().await;
                if !s.pushed {
                    s.pushed = true;
                    let push = PluginIpcResponse::DeferredCompleted {
                        task_id: "task-push".to_string(),
                        result: Ok(ToolResult::text("pushed result")),
                    };
                    if write_plugin_response(&mut stream, &push).await.is_err() {
                        break;
                    }
                }
            }

            if write_plugin_response(&mut stream, &resp).await.is_err() {
                break;
            }
        }
    }
}

/// Dispatches a single request to mock behavior.
///
/// `our_range` is the mock plugin's own advertised protocol version range,
/// mirroring the negotiation `ene-plugin`'s real `server.rs` performs.
async fn dispatch_mock(
    state: &Mutex<MockState>,
    req: PluginIpcRequest,
    our_range: VersionRange,
) -> PluginIpcResponse {
    match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            plugin_config,
            plugin_profiles,
            ..
        } => {
            // Mirrors `VersionRange::negotiate`: pick the highest common
            // version, or fail with a message naming both ranges when they
            // do not overlap.
            let Some(negotiated) = our_range.negotiate(&host_range) else {
                return PluginIpcResponse::Error {
                    request_id: String::new(),
                    message: format!(
                        "version mismatch: host supports {}-{}, mock supports {}-{}",
                        host_range.min, host_range.max, our_range.min, our_range.max
                    ),
                };
            };
            {
                let mut s = state.lock().await;
                s.plugin_config = plugin_config;
                s.plugin_profiles = plugin_profiles;
            }
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
        PluginIpcRequest::Ping { request_id } => PluginIpcResponse::Pong {
            request_id: request_id.clone(),
        },
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
                "mock.echo" | "mock.push" => PluginIpcResponse::CallResult {
                    request_id,
                    result: Ok(ToolResult::text(arguments)),
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
                        result: ToolResult::text("deferred result"),
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
        PluginIpcRequest::SetConfig {
            request_id,
            config,
            profiles,
        } => {
            let mut s = state.lock().await;
            s.plugin_config = Some(config);
            // Mirror plugin-server semantics: `None` clears live profiles.
            s.plugin_profiles = profiles.or_else(|| Some(serde_json::json!({})));
            PluginIpcResponse::ConfigApplied { request_id }
        }
        _ => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "unsupported request in mock".to_string(),
        },
    }
}

/// A releasable gate for deterministic concurrency tests.
///
/// `mock.slow` tool calls await [`Gate::wait`](Gate::wait) before responding;
/// a test releases them all at once with [`release`](Gate::release). This lets
/// tests assert on overlapping in-flight requests without any sleeps.
///
/// The gate can also be [`abort`](Gate::abort)ed: blocked calls then return
/// *without responding* so a test can simulate the plugin disappearing while
/// calls are in flight (the mock drops the connection instead of answering).
#[derive(Default)]
struct Gate {
    released: AtomicBool,
    aborted: AtomicBool,
    notify: Notify,
}

impl Gate {
    /// Blocks until the gate is released (returns `true`) or aborted (returns
    /// `false`).
    async fn wait(&self) -> bool {
        loop {
            if self.aborted.load(Ordering::Acquire) {
                return false;
            }
            if self.released.load(Ordering::Acquire) {
                return true;
            }
            self.notify.notified().await;
        }
    }

    /// Releases every current and future waiter.
    fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Aborts every current and future waiter, making them drop without
    /// responding.
    fn abort(&self) {
        self.aborted.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }
}

/// Waits (with a generous deadlock backstop) until `in_flight` reaches
/// `expected`. Polling on a short interval keeps the tests deterministic in
/// intent — the timeout only guards against a hang.
///
/// The counter is a plain [`AtomicUsize`] (not behind the state mutex), so
/// observing it never awaits: the observation path cannot itself contend with
/// the mock's request handling.
async fn wait_for_in_flight(in_flight: &AtomicUsize, expected: usize) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if in_flight.load(Ordering::Acquire) >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {expected} in-flight slow call(s)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// A mock plugin server that dispatches each request in its own spawned task,
/// mirroring the real `ene-plugin` server's concurrent read loop. A
/// dedicated writer lock serializes outgoing frames so concurrent handlers
/// never interleave bytes on the socket.
///
/// `mock.slow` tool calls block on `gate` until the test releases it, and
/// increment `in_flight` while blocked so tests can observe overlapping
/// in-flight requests. Every `mock.slow` dispatch also increments
/// `call_count`, so a test can assert each request reached the plugin exactly
/// once (no duplicate replay across a reconnect).
async fn run_concurrent_mock_server(
    socket_path: PathBuf,
    state: Arc<Mutex<MockState>>,
    in_flight: Arc<AtomicUsize>,
    call_count: Arc<AtomicUsize>,
    gate: Arc<Gate>,
) {
    cleanup_path(&socket_path);
    let mut listener = IpcListener::bind(&socket_path).expect("failed to bind mock server");

    let Ok(stream) = listener.accept().await else {
        return;
    };
    let (read_half, write_half) = tokio::io::split(stream);
    let mut reader = read_half;
    let writer = Arc::new(Mutex::new(write_half));

    loop {
        let Ok(Some(req)) = read_plugin_request(&mut reader).await else {
            break;
        };
        let state = Arc::clone(&state);
        let in_flight = Arc::clone(&in_flight);
        let call_count = Arc::clone(&call_count);
        let gate = Arc::clone(&gate);
        let writer = Arc::clone(&writer);
        tokio::spawn(async move {
            let Some(resp) =
                dispatch_concurrent_mock(&state, &in_flight, &call_count, &gate, req).await
            else {
                // The dispatch chose not to respond (e.g. the gate was aborted
                // to simulate the plugin disappearing mid-call); drop the
                // request without writing a frame.
                return;
            };
            let mut w = writer.lock().await;
            // A write failure only means the host dropped the connection; the
            // mock has nothing to do but stop responding to this request.
            drop(write_plugin_response(&mut *w, &resp).await);
        });
    }
}

/// Dispatch for [`run_concurrent_mock_server`]: handles the handshake, ping,
/// and the `mock.slow` / `mock.echo` tool calls used by the concurrency tests.
///
/// Returns `None` when a `mock.slow` call is aborted at the gate, signalling
/// the server to drop the request without responding (simulating a plugin that
/// disappears while the call is in flight).
async fn dispatch_concurrent_mock(
    _state: &Mutex<MockState>,
    in_flight: &AtomicUsize,
    call_count: &AtomicUsize,
    gate: &Gate,
    req: PluginIpcRequest,
) -> Option<PluginIpcResponse> {
    Some(match req {
        PluginIpcRequest::Handshake {
            version: host_range,
            ..
        } => {
            let our_range = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            };
            let negotiated = our_range
                .negotiate(&host_range)
                .expect("test ranges must overlap");
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
        PluginIpcRequest::Ping { request_id } => PluginIpcResponse::Pong { request_id },
        PluginIpcRequest::CallTool {
            request_id,
            name,
            arguments,
            ..
        } => match name.as_str() {
            "mock.slow" => {
                call_count.fetch_add(1, Ordering::AcqRel);
                in_flight.fetch_add(1, Ordering::AcqRel);
                let respond = gate.wait().await;
                in_flight.fetch_sub(1, Ordering::AcqRel);
                if !respond {
                    // Aborted at the gate: drop the request without responding,
                    // simulating a plugin that disappears mid-call.
                    return None;
                }
                PluginIpcResponse::CallResult {
                    request_id,
                    result: Ok(ToolResult::text(arguments)),
                }
            }
            "mock.echo" => PluginIpcResponse::CallResult {
                request_id,
                result: Ok(ToolResult::text(arguments)),
            },
            _ => PluginIpcResponse::CallResult {
                request_id,
                result: Err(ToolError::NotFound { tool_name: name }),
            },
        },
        _ => PluginIpcResponse::Error {
            request_id: String::new(),
            message: "unsupported request in concurrent mock".to_string(),
        },
    })
}

/// Spawns a concurrent mock server and connects an [`IpcPluginConnection`]
/// with the given concurrency bound. Returns the connection, the shared mock
/// state, the in-flight counter for `mock.slow`, the dispatch counter for
/// `mock.slow`, the gate controlling `mock.slow`, the server task handle (so a
/// test can abort the accept loop to simulate the plugin process going away),
/// and the socket path.
async fn spawn_concurrent_and_connect(
    name: &str,
    max_concurrent: usize,
) -> (
    Arc<IpcPluginConnection>,
    Arc<Mutex<MockState>>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<Gate>,
    tokio::task::JoinHandle<()>,
    PathBuf,
) {
    let socket_path = test_socket_path(name);
    let state = Arc::new(Mutex::new(MockState::default()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let call_count = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Gate::default());

    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    let server_in_flight = Arc::clone(&in_flight);
    let server_call_count = Arc::clone(&call_count);
    let server_gate = Arc::clone(&gate);
    let server_handle = tokio::spawn(async move {
        run_concurrent_mock_server(
            server_path,
            server_state,
            server_in_flight,
            server_call_count,
            server_gate,
        )
        .await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        max_concurrent,
    )
    .await
    .expect("handshake should succeed");

    (
        Arc::new(conn),
        state,
        in_flight,
        call_count,
        gate,
        server_handle,
        socket_path,
    )
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
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake should succeed");

    (conn, state, socket_path)
}

/// Spawns a mock server that declares `plugin_range` during the handshake
/// (rather than always the current [`PLUGIN_IPC_PROTOCOL_VERSION`]) and
/// attempts to connect, returning the raw [`Result`] so both successful and
/// failing negotiations can be asserted on.
async fn try_connect_with_plugin_version(
    name: &str,
    plugin_range: VersionRange,
) -> (Result<IpcPluginConnection, PluginHostError>, PathBuf) {
    let socket_path = test_socket_path(name);
    let state = Arc::new(Mutex::new(MockState::default()));

    let server_path = socket_path.clone();
    tokio::spawn(async move {
        run_mock_server_with_version(server_path, state, plugin_range).await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let result = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await;

    (result, socket_path)
}

// ── Test: Handshake ──────────────────────────────────────────────────────

#[tokio::test]
async fn handshake_succeeds_and_returns_capabilities() {
    let (conn, _state, socket_path) = spawn_and_connect("handshake").await;

    let caps = conn.capabilities();
    assert_eq!(caps.tools, 1);
    assert!(caps.llm_providers.is_empty());

    // Verify ListTools returns the actual spec.
    let tools = conn.list_tools().await.expect("list_tools should succeed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name.as_str(), "mock.echo");

    // The handshake negotiated the current protocol version (mock and host
    // both advertise it by default), so the negotiated-version accessor and
    // the v5 `SetConfig` feature gate agree.
    assert_eq!(conn.negotiated_version(), PLUGIN_IPC_PROTOCOL_VERSION);
    assert!(conn.supports_cancel_stream());
    assert!(conn.supports_set_config());

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn handshake_delivers_config_and_profiles() {
    // The host passes `plugins.list.<name>.config` and `.profiles` through
    // the handshake; the mock records what it received so host→plugin
    // delivery is pinned on the host side.
    let socket_path = test_socket_path("config-profiles");
    let state = Arc::new(Mutex::new(MockState::default()));

    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });

    // Give the server a moment to bind.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let config = serde_json::json!({"api_key": {"source": "env"}});
    let profiles = serde_json::json!({
        "kokoro": {"voices_path": "/data/voices.bin"}
    });
    let _conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        Some(config.clone()),
        Some(profiles.clone()),
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake should succeed");

    let s = state.lock().await;
    assert_eq!(
        s.plugin_config,
        Some(config),
        "config blob must reach the plugin handshake"
    );
    assert_eq!(
        s.plugin_profiles,
        Some(profiles),
        "profiles blob must reach the plugin handshake"
    );
    drop(s);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn set_config_pushes_to_live_plugin() {
    let (conn, state, socket_path) = spawn_and_connect("set-config-live").await;

    let updated = serde_json::json!({"api_key": "sk-hot-reload"});
    let profiles = serde_json::json!({"p": {"v": 2}});
    conn.set_config(Some(updated.clone()), Some(profiles.clone()))
        .await
        .expect("SetConfig should succeed on a v5 plugin");

    let s = state.lock().await;
    assert_eq!(
        s.plugin_config.as_ref(),
        Some(&updated),
        "live SetConfig must update the mock's recorded config"
    );
    assert_eq!(
        s.plugin_profiles.as_ref(),
        Some(&profiles),
        "live SetConfig must update the mock's recorded profiles"
    );
    drop(s);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn set_config_none_profiles_clears_live_plugin_profiles() {
    let (conn, state, socket_path) = spawn_and_connect("set-config-clear-profiles").await;

    conn.set_config(
        Some(serde_json::json!({"api_key": "sk"})),
        Some(serde_json::json!({"p": {"v": 1}})),
    )
    .await
    .expect("initial SetConfig");

    conn.set_config(Some(serde_json::json!({"api_key": "sk"})), None)
        .await
        .expect("clearing SetConfig");

    let s = state.lock().await;
    assert_eq!(
        s.plugin_profiles.as_ref(),
        Some(&serde_json::json!({})),
        "None profiles on SetConfig must clear, not leave the previous map"
    );
    drop(s);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn set_config_updates_cache_used_on_reconnect() {
    // After a live SetConfig, a transport-driven reconnect must re-handshake
    // with the updated blob rather than the original connect-time value.
    let socket_path = test_socket_path("set-config-reconnect");
    let state = Arc::new(Mutex::new(MockState::default()));

    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    let server = tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let initial = serde_json::json!({"v": 1});
    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        Some(initial),
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake should succeed");

    let updated = serde_json::json!({"v": 2});
    conn.set_config(Some(updated.clone()), None)
        .await
        .expect("SetConfig should succeed");

    // Kill the server so the next request hits a transport failure and
    // reconnects; restart it on the same path before the reconnect retries
    // exhaust.
    server.abort();
    cleanup_path(&socket_path);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Clear recorded handshake state so we can assert the reconnect payload.
    {
        let mut s = state.lock().await;
        s.plugin_config = None;
        s.plugin_profiles = None;
    }

    conn.list_tools()
        .await
        .expect("list_tools after reconnect should succeed");

    let s = state.lock().await;
    assert_eq!(
        s.plugin_config.as_ref(),
        Some(&updated),
        "reconnect handshake must deliver the SetConfig-updated blob"
    );
    drop(s);

    cleanup_path(&socket_path);
}

// ── Test: Handshake timeout ─────────────────────────────────────────────

#[tokio::test]
async fn handshake_times_out_when_plugin_never_responds() {
    // A plugin that binds its listener and accepts the socket but never
    // replies to the `Handshake` request must not hang `connect()` forever.
    // The host applies a bounded timeout and surfaces a `HandshakeFailed`
    // error instead, so one wedged plugin cannot block startup of the rest.
    let socket_path = test_socket_path("handshake-timeout");
    cleanup_path(&socket_path);
    let mut listener = IpcListener::bind(&socket_path).expect("failed to bind silent server");

    // Accept the connection but never write a response, simulating a plugin
    // stuck in heavy pre-handshake initialization.
    tokio::spawn(async move {
        let Ok(_stream) = listener.accept().await else {
            return;
        };
        // Hold the stream open without responding until the test drops it.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // A short timeout keeps the test fast; the important property is that
    // `connect` returns (with an error) rather than blocking indefinitely.
    let short_timeout = std::time::Duration::from_millis(300);
    let started = std::time::Instant::now();
    let result = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        short_timeout,
        TEST_MAX_CONCURRENT,
    )
    .await;
    let elapsed = started.elapsed();

    let Err(err) = result else {
        panic!("a silent plugin must fail the handshake, but connect() succeeded");
    };
    assert!(
        matches!(err, PluginHostError::HandshakeFailed { .. }),
        "expected HandshakeFailed, got {err:?}"
    );
    // The error message must make the timeout nature explicit.
    assert!(
        err.to_string().contains("no HandshakeAck within"),
        "diagnostic should mention the handshake timeout, got: {err}"
    );
    // The failure must be bounded by (roughly) the configured timeout, not
    // the 30 s the silent server holds the socket open.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "connect returned too slowly ({elapsed:?}); the timeout is not bounding the wait"
    );

    cleanup_path(&socket_path);
}

// ── Test: N-1 backward compatibility (issue #275) ────────────────────────

#[tokio::test]
async fn handshake_negotiates_min_supported_version_for_older_plugin() {
    // A plugin built against the oldest version the host still supports
    // declares `{min: N-1, max: N-1}` — mirroring how a real out-of-process
    // plugin binary that hasn't been rebuilt against the newest
    // `ene-plugin-proto` would behave. The host's range
    // (`VersionRange::host_supported()`) spans `[N-1, N]`, so negotiation
    // must succeed at exactly N-1, the highest version common to both.
    let old_plugin_range = VersionRange {
        min: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
        max: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
    };
    let (result, socket_path) =
        try_connect_with_plugin_version("old-plugin", old_plugin_range).await;

    let conn = result.expect("handshake with an N-1 plugin should succeed");
    assert_eq!(conn.negotiated_version(), PLUGIN_IPC_MIN_SUPPORTED_VERSION);

    // N-1 is currently v4, so CancelStream is available, but SetConfig (v5)
    // is not — the gate must report it unsupported and `set_config` must
    // update the local cache without sending a message the plugin cannot
    // deserialize.
    assert!(conn.supports_cancel_stream());
    assert!(!conn.supports_set_config());
    conn.set_config(Some(serde_json::json!({"k": 1})), None)
        .await
        .expect("set_config must no-op IPC instead of erroring on an old plugin");

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn handshake_below_supported_floor_fails_with_both_ranges_in_diagnostic() {
    // A plugin two versions behind the host (below `host_supported().min`)
    // cannot negotiate any common version. The resulting diagnostic must
    // name both the host's supported range and the plugin's offered range
    // so a developer can tell the plugin needs rebuilding.
    let ancient_range = VersionRange {
        min: PLUGIN_IPC_MIN_SUPPORTED_VERSION - 1,
        max: PLUGIN_IPC_MIN_SUPPORTED_VERSION - 1,
    };
    let (result, socket_path) =
        try_connect_with_plugin_version("ancient-plugin", ancient_range).await;

    let Err(err) = result else {
        panic!("handshake below the supported floor must fail");
    };
    let message = err.to_string();

    let host_range = VersionRange::host_supported();
    assert!(
        message.contains(&host_range.min.to_string())
            && message.contains(&host_range.max.to_string()),
        "diagnostic must include the host's supported range: {message}"
    );
    assert!(
        message.contains(&ancient_range.min.to_string()),
        "diagnostic must include the plugin's offered range: {message}"
    );
    assert!(
        matches!(err, PluginHostError::HandshakeFailed { .. }),
        "expected HandshakeFailed, got: {err:?}"
    );

    cleanup_path(&socket_path);
}

// ── Test: call_tool round-trip ───────────────────────────────────────────

#[tokio::test]
async fn call_tool_round_trip() {
    let (conn, _state, socket_path) = spawn_and_connect("call-tool").await;

    let result = conn
        .call_tool("mock.echo", r#"{"msg":"hello"}"#, None)
        .await
        .expect("call_tool should succeed");
    assert_eq!(result.text_for_llm(), r#"{"msg":"hello"}"#);

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn call_tool_not_found() {
    let (conn, _state, socket_path) = spawn_and_connect("call-tool-nf").await;

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
    let (conn, _state, socket_path) = spawn_and_connect("perm-required").await;

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
    let (conn, _state, socket_path) = spawn_and_connect("user-input").await;

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
    let (conn, state, socket_path) = spawn_and_connect("call-ctx").await;

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
    let (conn, state, socket_path) = spawn_and_connect("approve").await;

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
    let (conn, state, socket_path) = spawn_and_connect("patterns").await;

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
    let (conn, _state, socket_path) = spawn_and_connect("deferred").await;

    let outcome = conn
        .call_tool_deferred("mock.deferred", "{}", None)
        .await
        .expect("call_tool_deferred should succeed");

    match outcome {
        ene_plugin_proto::DeferredOutcome::Deferred { task_id } => {
            assert_eq!(task_id, "task-42");
        }
        ene_plugin_proto::DeferredOutcome::Sync(other) => {
            panic!("expected Deferred, got Sync: {other:?}")
        }
    }

    let status = conn
        .poll_deferred("task-42")
        .await
        .expect("poll_deferred should succeed");
    assert_eq!(
        status,
        DeferredStatus::Completed {
            result: ToolResult::text("deferred result")
        }
    );

    cleanup_path(&socket_path);
}

#[tokio::test]
async fn deferred_call_sync_fallback() {
    let (conn, _state, socket_path) = spawn_and_connect("deferred-sync").await;

    // mock.echo does not support deferred, so the server falls back to sync.
    let outcome = conn
        .call_tool_deferred("mock.echo", r#"{"x":1}"#, None)
        .await
        .expect("call_tool_deferred should succeed with sync fallback");

    match outcome {
        ene_plugin_proto::DeferredOutcome::Sync(result) => {
            assert_eq!(result.text_for_llm(), r#"{"x":1}"#);
        }
        ene_plugin_proto::DeferredOutcome::Deferred { task_id } => {
            panic!("expected Sync, got Deferred: {task_id:?}")
        }
    }

    cleanup_path(&socket_path);
}

// ── Test: DeferredCompleted push routing (Cr-5 / H-12) ───────────────────

#[tokio::test]
async fn deferred_completed_push_is_routed_without_breaking_correlation() {
    let (conn, state, socket_path) = spawn_and_connect("deferred-push").await;

    // `mock.push` makes the server emit a `DeferredCompleted` push frame
    // *ahead* of the normal `CallResult`. The single reader task must route
    // the push into the completion cache and still correlate the `CallResult`
    // with this request (Cr-5 / H-12).
    let result = conn
        .call_tool("mock.push", r#"{"x":1}"#, None)
        .await
        .expect("call_tool should still correlate its CallResult");
    assert_eq!(result.text_for_llm(), r#"{"x":1}"#);

    // The push was cached; polling the pushed task returns the pushed result
    // without any further round-trip to the plugin.
    let status = conn
        .poll_deferred("task-push")
        .await
        .expect("poll_deferred should return the cached push");
    assert_eq!(
        status,
        DeferredStatus::Completed {
            result: ToolResult::text("pushed result")
        }
    );

    // The push was emitted exactly once.
    assert!(state.lock().await.pushed);

    cleanup_path(&socket_path);
}

// ── Test: cancel_deferred ────────────────────────────────────────────────

#[tokio::test]
async fn cancel_deferred_reaches_plugin() {
    let (conn, state, socket_path) = spawn_and_connect("cancel").await;

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
    let (conn, _state, socket_path) = spawn_and_connect("ping").await;

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

    let conn = IpcPluginConnection::connect(
        &socket_path,
        SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
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

// ── Test: Request multiplexing ────────────────────────────────────

/// Two slow tool calls against the *same* connection must be in flight
/// concurrently: both reach the plugin (the gate observes two in-flight) before
/// either is released. Under the old per-connection serialization the second
/// call could not even be written until the first completed, so the in-flight
/// count would never exceed one.
#[tokio::test]
async fn two_slow_tool_calls_overlap_in_flight() {
    let (conn, _state, in_flight, _call_count, gate, _server, socket_path) =
        spawn_concurrent_and_connect("overlap", 8).await;

    let c1 = Arc::clone(&conn);
    let t1 = tokio::spawn(async move { c1.call_tool("mock.slow", "one", None).await });
    let c2 = Arc::clone(&conn);
    let t2 = tokio::spawn(async move { c2.call_tool("mock.slow", "two", None).await });

    // Both requests must reach the plugin and be in flight at the same time.
    wait_for_in_flight(&in_flight, 2).await;

    // Release both and collect results.
    gate.release();
    let (r1, r2) = (t1.await.expect("t1 join"), t2.await.expect("t2 join"));
    assert_eq!(r1.expect("call one").text_for_llm(), "one");
    assert_eq!(r2.expect("call two").text_for_llm(), "two");

    cleanup_path(&socket_path);
}

/// A `ping` must complete while a slow tool call is still pending. The probe
/// shares the connection but is not queued behind the in-flight call — neither
/// by a host-side lock (there is none on the request path) nor by the plugin
/// (which dispatches each request concurrently).
#[tokio::test]
async fn ping_completes_while_slow_call_pending() {
    let (conn, _state, in_flight, _call_count, gate, _server, socket_path) =
        spawn_concurrent_and_connect("ping-during", 8).await;

    let c1 = Arc::clone(&conn);
    let slow = tokio::spawn(async move { c1.call_tool("mock.slow", "slow", None).await });

    // Wait until the slow call is in flight, then ping. The ping must return
    // Ok *before* the gate is released (the slow call is still blocked).
    wait_for_in_flight(&in_flight, 1).await;
    conn.ping()
        .await
        .expect("ping must succeed while a slow call is pending");

    gate.release();
    assert_eq!(
        slow.await.expect("join").expect("call").text_for_llm(),
        "slow"
    );

    cleanup_path(&socket_path);
}

/// The connection-level concurrency bound (sourced from `max_concurrent`) caps
/// in-flight requests: with a bound of 1, a second slow call cannot reach the
/// plugin until the first completes. This is the host-side protection that
/// keeps the plugin from being flooded.
#[tokio::test]
async fn in_flight_requests_are_bounded_by_max_concurrent() {
    // Bound of 1: strictly serial in-flight.
    let (conn, _state, in_flight, _call_count, gate, _server, socket_path) =
        spawn_concurrent_and_connect("bounded", 1).await;

    let c1 = Arc::clone(&conn);
    let t1 = tokio::spawn(async move { c1.call_tool("mock.slow", "one", None).await });

    // The first call occupies the single permit and reaches the plugin.
    wait_for_in_flight(&in_flight, 1).await;

    // A second call cannot acquire a permit, so it never reaches the plugin:
    // the in-flight count stays at exactly 1 while the gate is held.
    let c2 = Arc::clone(&conn);
    let t2 = tokio::spawn(async move { c2.call_tool("mock.slow", "two", None).await });

    // Give t2 a chance to (incorrectly, if unbounded) reach the plugin. The
    // in-flight count must remain 1, proving the second call is queued on the
    // semaphore rather than dispatched. This 50 ms sleep proves a negative
    // (nothing happened) and is deliberately generous: it is a safety margin
    // against scheduler jitter, not the mechanism under test — the real
    // guarantee is the semaphore, asserted by the count staying at 1.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        in_flight.load(Ordering::Acquire),
        1,
        "bound of 1 must keep exactly one request in flight"
    );

    // Release the first; the second may now proceed.
    gate.release();
    assert_eq!(t1.await.expect("join").expect("call").text_for_llm(), "one");
    assert_eq!(t2.await.expect("join").expect("call").text_for_llm(), "two");

    cleanup_path(&socket_path);
}

/// Dropping the plugin while N calls are in flight must fail every caller
/// **promptly** — well inside the 2-minute `DEFAULT_TIMEOUT` — rather than
/// leaving the waiters orphaned until their own timeout.
///
/// `reconnect_from` aborts the old reader task, which is suspended inside
/// `read_plugin_response` and therefore never reaches the trailing `fail_all()`
/// in `reader_loop`; the reconnect must fail the waiters itself (item 1). And
/// because waiter registration is atomic with the frame write under the writer
/// lock (item 3), no request can be written to the fresh stream and then
/// replayed by the retry path — so the plugin observes each request exactly
/// once, preserving the "a transport error means the request never reached the
/// plugin" invariant for non-idempotent `CallTool`.
///
/// Without the item 1 fix the callers would block for the full `DEFAULT_TIMEOUT`
/// (the mock never comes back, so the reconnect cannot succeed) and this test
/// would fail on its deadline.
#[tokio::test]
async fn in_flight_calls_fail_promptly_when_plugin_drops() {
    let (conn, _state, in_flight, call_count, gate, server_handle, socket_path) =
        spawn_concurrent_and_connect("drop-inflight", 8).await;

    const N: usize = 3;
    let mut handles = Vec::new();
    for i in 0..N {
        let c = Arc::clone(&conn);
        handles.push(tokio::spawn(async move {
            c.call_tool("mock.slow", &format!("call-{i}"), None).await
        }));
    }

    // All N calls must reach the plugin and be blocked on the gate at once.
    wait_for_in_flight(&in_flight, N).await;
    assert_eq!(
        call_count.load(Ordering::Acquire),
        N,
        "each request must reach the plugin exactly once before the drop"
    );

    // Simulate the plugin process disappearing while the calls are in flight:
    // abort the gate so the in-flight dispatch tasks return without responding
    // (dropping their writer clones), then abort the accept-loop task (dropping
    // the reader half and the last writer Arc). With every half of the
    // connection gone, the host's reader observes EOF.
    gate.abort();
    server_handle.abort();
    cleanup_path(&socket_path);

    // Every caller must fail far inside DEFAULT_TIMEOUT (2 min). On observing
    // the transport failure each caller attempts a reconnect that cannot
    // succeed (nothing is listening), running the full connect-retry budget
    // (~2.5 s) before surfacing the error. Those reconnects serialize on the
    // writer lock and the generation does not advance on a *failed* reconnect,
    // so the last of the N callers returns after roughly N × 2.5 s. The bound
    // below accommodates that with margin while staying an order of magnitude
    // under DEFAULT_TIMEOUT — without the item 1 `fail_all()` fix the waiters
    // would instead block for the full 2 min and blow well past this deadline.
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(2_500 * N as u64 + 10_000);
    for handle in handles {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let result = tokio::time::timeout(remaining, handle)
            .await
            .expect("caller must return promptly, not block until DEFAULT_TIMEOUT")
            .expect("task must not panic");
        assert!(
            result.is_err(),
            "in-flight call must fail once the plugin drops"
        );
    }

    // The plugin observed each request exactly once: no request was replayed to
    // a fresh stream by the retry path (item 3).
    assert_eq!(
        call_count.load(Ordering::Acquire),
        N,
        "no request may be dispatched to the plugin more than once"
    );

    cleanup_path(&socket_path);
}

/// Concurrent transport failures must coalesce into a single reconnect.
///
/// Two `mock.echo` calls are issued back to back against a plugin whose socket
/// has been removed, so both writes fail with a transport error and both enter
/// the reconnect path. `reconnect_from` re-checks the generation *under the
/// writer lock*, so only the first caller actually reconnects; the second
/// observes the advanced generation and returns without tearing the connection
/// down again. With the server restarted, both retries then succeed on the
/// single fresh connection.
///
/// Without the under-lock re-check the second reconnect would abort the reader
/// task the first reconnect just spawned and `fail_all()` its already-retried
/// sibling, surfacing as an error here instead of two successes.
#[tokio::test]
async fn concurrent_transport_failures_coalesce_into_one_reconnect() {
    let socket_path = test_socket_path("coalesce");
    let state = Arc::new(Mutex::new(MockState::default()));

    // Phase 1: start server, connect, confirm a healthy round-trip.
    let server_path = socket_path.clone();
    let server_state = Arc::clone(&state);
    let server_handle = tokio::spawn(async move {
        run_mock_server(server_path, server_state).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let conn = Arc::new(
        IpcPluginConnection::connect(
            &socket_path,
            SandboxConfigData::default(),
            None,
            None,
            TEST_HANDSHAKE_TIMEOUT,
            TEST_MAX_CONCURRENT,
        )
        .await
        .expect("initial handshake should succeed"),
    );
    conn.call_tool("mock.echo", "warmup", None)
        .await
        .expect("warmup call should succeed");

    // Phase 2: kill the server and remove the socket so in-flight writes fail.
    server_handle.abort();
    cleanup_path(&socket_path);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Phase 3: restart the server on the same path so the coalesced reconnect
    // can succeed.
    let server_path2 = socket_path.clone();
    let server_state2 = Arc::clone(&state);
    tokio::spawn(async move {
        run_mock_server(server_path2, server_state2).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Phase 4: two concurrent calls both fail their first write and race into
    // the reconnect path. Exactly one reconnect must occur; both must then
    // succeed on the fresh connection.
    let c1 = Arc::clone(&conn);
    let t1 = tokio::spawn(async move { c1.call_tool("mock.echo", "a", None).await });
    let c2 = Arc::clone(&conn);
    let t2 = tokio::spawn(async move { c2.call_tool("mock.echo", "b", None).await });

    let (r1, r2) = (t1.await.expect("t1 join"), t2.await.expect("t2 join"));
    assert_eq!(
        r1.expect("call a must succeed after coalesced reconnect")
            .text_for_llm(),
        "a"
    );
    assert_eq!(
        r2.expect("call b must succeed after coalesced reconnect")
            .text_for_llm(),
        "b"
    );

    cleanup_path(&socket_path);
}
