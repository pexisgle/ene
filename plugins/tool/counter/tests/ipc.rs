//! End-to-end IPC integration tests for the counter plugin binary.
//!
//! These tests spawn the real `ene-plugin-counter` binary and drive it
//! over the wire protocol: handshake, spec listing, argument validation,
//! not-found dispatch, and the permission flow.
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use expect/panic for assertions"
)]

use ene_plugin_proto::{
    CallContext, IpcStream, PLUGIN_IPC_PROTOCOL_VERSION, PluginCapabilities, PluginIpcRequest,
    PluginIpcResponse, SandboxConfigData, ToolError, VersionRange, WireFormat, cleanup_path,
    read_plugin_response, write_plugin_request,
};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/ene-plugin-counter-test-{}-{id}-{name}.sock",
        std::process::id()
    ))
}

/// Spawned plugin binary plus its socket; the child is killed and the
/// socket removed when the test finishes.
struct PluginProcess {
    child: Child,
    socket_path: PathBuf,
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
        cleanup_path(&self.socket_path);
    }
}

fn spawn_plugin(name: &str) -> PluginProcess {
    let socket_path = test_socket_path(name);
    cleanup_path(&socket_path);
    let mut child = Command::new(env!("CARGO_BIN_EXE_ene-plugin-counter"))
        .env("ENE_PLUGIN_SOCKET", &socket_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn plugin binary");

    for _ in 0..100 {
        if socket_path.exists() {
            return PluginProcess { child, socket_path };
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(child.kill());
    drop(child.wait());
    panic!(
        "plugin binary did not create socket {}",
        socket_path.display()
    );
}

/// Raw-stream test peer that tracks the negotiated wire format, mirroring
/// how the host switches framing after the handshake ack.
struct TestPeer<'a> {
    stream: &'a mut IpcStream,
    format: WireFormat,
}

impl TestPeer<'_> {
    fn new(stream: &mut IpcStream) -> TestPeer<'_> {
        TestPeer {
            stream,
            format: WireFormat::Json,
        }
    }

    async fn handshake(&mut self) -> PluginCapabilities {
        write_plugin_request(
            self.stream,
            &PluginIpcRequest::Handshake {
                version: VersionRange {
                    min: PLUGIN_IPC_PROTOCOL_VERSION,
                    max: PLUGIN_IPC_PROTOCOL_VERSION,
                },
                sandbox: SandboxConfigData::default(),
                plugin_config: None,
                plugin_profiles: None,
            },
            WireFormat::Json,
        )
        .await
        .expect("write handshake");

        let resp = read_plugin_response(self.stream, WireFormat::Json)
            .await
            .expect("read handshake ack")
            .expect("non-EOF");

        match resp {
            PluginIpcResponse::HandshakeAck {
                version,
                capabilities,
            } => {
                assert_eq!(version, PLUGIN_IPC_PROTOCOL_VERSION);
                self.format = WireFormat::for_version(version);
                capabilities
            }
            other => panic!("expected HandshakeAck, got: {other:?}"),
        }
    }

    async fn round_trip(&mut self, req: &PluginIpcRequest) -> PluginIpcResponse {
        write_plugin_request(self.stream, req, self.format)
            .await
            .expect("write request");
        read_plugin_response(self.stream, self.format)
            .await
            .expect("read response")
            .expect("non-EOF")
    }
}

fn call_tool(name: &str, arguments: &str, request_id: &str) -> PluginIpcRequest {
    PluginIpcRequest::CallTool {
        request_id: request_id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
        deferred: false,
        context: Some(CallContext {
            conversation_id: "conv-1".to_string(),
            turn_id: "turn-1".to_string(),
        }),
    }
}

#[tokio::test]
async fn handshake_advertises_three_tools() {
    let process = spawn_plugin("hs");
    let mut stream = IpcStream::connect(&process.socket_path)
        .await
        .expect("connect to plugin socket");
    let mut peer = TestPeer::new(&mut stream);
    let caps = peer.handshake().await;
    assert_eq!(caps.tools, 3);
}

#[tokio::test]
async fn list_specs_returns_counter_actions() {
    let process = spawn_plugin("specs");
    let mut stream = IpcStream::connect(&process.socket_path)
        .await
        .expect("connect to plugin socket");
    let mut peer = TestPeer::new(&mut stream);
    peer.handshake().await;

    let resp = peer
        .round_trip(&PluginIpcRequest::ListTools {
            request_id: "req-list".to_string(),
        })
        .await;

    match resp {
        PluginIpcResponse::Tools { tools, .. } => {
            let names: Vec<&str> = tools.iter().map(|spec| spec.name.as_str()).collect();
            assert_eq!(names, ["counter.get", "counter.increment", "counter.reset"]);
        }
        other => panic!("expected Tools, got: {other:?}"),
    }
}

#[tokio::test]
async fn malformed_arguments_are_rejected_over_ipc() {
    let process = spawn_plugin("badargs");
    let mut stream = IpcStream::connect(&process.socket_path)
        .await
        .expect("connect to plugin socket");
    let mut peer = TestPeer::new(&mut stream);
    peer.handshake().await;

    let resp = peer
        .round_trip(&call_tool("counter.get", "not json", "req-1"))
        .await;

    match resp {
        PluginIpcResponse::CallResult {
            result: Err(ToolError::InvalidArguments { .. }),
            ..
        } => {}
        other => panic!("expected InvalidArguments, got: {other:?}"),
    }
}

#[tokio::test]
async fn unknown_tool_is_not_found_over_ipc() {
    let process = spawn_plugin("notfound");
    let mut stream = IpcStream::connect(&process.socket_path)
        .await
        .expect("connect to plugin socket");
    let mut peer = TestPeer::new(&mut stream);
    peer.handshake().await;

    let resp = peer
        .round_trip(&call_tool("counter.nope", "{}", "req-1"))
        .await;

    match resp {
        PluginIpcResponse::CallResult {
            result: Err(ToolError::NotFound { .. }),
            ..
        } => {}
        other => panic!("expected NotFound, got: {other:?}"),
    }
}

#[tokio::test]
async fn permission_flow_over_ipc() {
    let process = spawn_plugin("perm");
    let mut stream = IpcStream::connect(&process.socket_path)
        .await
        .expect("connect to plugin socket");
    let mut peer = TestPeer::new(&mut stream);
    peer.handshake().await;

    // Without approval the destructive action must ask the user first.
    let resp = peer
        .round_trip(&call_tool("counter.reset", "{}", "req-1"))
        .await;
    let permission_request_id = match resp {
        PluginIpcResponse::CallResult {
            result:
                Err(ToolError::PermissionRequired {
                    request_id, action, ..
                }),
            ..
        } => {
            assert_eq!(action, "CounterReset");
            request_id
        }
        other => panic!("expected PermissionRequired, got: {other:?}"),
    };

    // Approving the request must let the retried call pass the gate.
    let resp = peer
        .round_trip(&PluginIpcRequest::ApprovePermission {
            request_id: "req-2".to_string(),
            permission_request_id,
        })
        .await;
    match resp {
        PluginIpcResponse::Ack { .. } => {}
        other => panic!("expected Ack, got: {other:?}"),
    }

    // The retried call reaches the store layer; with no DB server in the
    // test the sandbox handshake never provided a socket, so it fails as
    // an internal error — proving the approval was consumed and the gate
    // no longer blocks.
    let resp = peer
        .round_trip(&call_tool("counter.reset", "{}", "req-3"))
        .await;
    match resp {
        PluginIpcResponse::CallResult {
            result:
                Err(ToolError::Generic {
                    kind: ene_plugin_proto::ErrorKind::Internal,
                    ..
                }),
            ..
        } => {}
        other => panic!("expected internal DB error after approval, got: {other:?}"),
    }
}
