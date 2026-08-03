//! Capability-declaration contract tests over the real IPC handshake.
//!
//! A mock plugin advertises `provides` / `requires` in its `HandshakeAck`;
//! the host must receive them verbatim through [`IpcPluginConnection`] and
//! feed them into the capability registry, so the registry's resolution
//! contract is exercised against the exact bytes a plugin sends.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests use unwrap/expect for assertions"
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use ene_plugin_host::{CapabilityRegistry, IpcPluginConnection};
use ene_plugin_proto::{
    CapabilityRef, CapabilityRequirement, IpcListener, PLUGIN_IPC_PROTOCOL_VERSION,
    PluginCapabilities, PluginIpcRequest, PluginIpcResponse, VersionRange, WireFormat,
    cleanup_path, read_plugin_request, write_plugin_response,
};

/// Counter for generating unique socket paths across parallel tests.
static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Handshake timeout used by the integration tests.
const TEST_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Concurrency bound passed to [`IpcPluginConnection::connect`] in tests.
const TEST_MAX_CONCURRENT: usize = 8;

/// Returns a unique socket path for a test.
fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!(
        "/tmp/ene-capability-test-{}-{id}-{name}.sock",
        std::process::id()
    ))
}

/// Serves the handshake with `capabilities` and answers pings, on a fresh
/// connection per accept until the listener is dropped.
async fn run_mock_server(socket_path: PathBuf, capabilities: PluginCapabilities) {
    cleanup_path(&socket_path);
    let Ok(mut listener) = IpcListener::bind(&socket_path) else {
        return;
    };
    loop {
        let Ok(mut stream) = listener.accept().await else {
            break;
        };
        let capabilities = capabilities.clone();
        tokio::spawn(async move {
            let Ok(Some(PluginIpcRequest::Handshake {
                version: host_range,
                ..
            })) = read_plugin_request(&mut stream, WireFormat::Json).await
            else {
                return;
            };
            let negotiated = VersionRange {
                min: PLUGIN_IPC_PROTOCOL_VERSION,
                max: PLUGIN_IPC_PROTOCOL_VERSION,
            }
            .negotiate(&host_range)
            .unwrap_or(PLUGIN_IPC_PROTOCOL_VERSION);
            if write_plugin_response(
                &mut stream,
                &PluginIpcResponse::HandshakeAck {
                    version: negotiated,
                    capabilities,
                },
                WireFormat::Json,
            )
            .await
            .is_err()
            {
                return;
            }
            // Answer pings so the connection stays healthy for the test.
            while let Ok(Some(PluginIpcRequest::Ping { request_id })) =
                read_plugin_request(&mut stream, WireFormat::Json).await
            {
                if write_plugin_response(
                    &mut stream,
                    &PluginIpcResponse::Pong { request_id },
                    WireFormat::Json,
                )
                .await
                .is_err()
                {
                    break;
                }
            }
        });
    }
}

/// Spawns a mock server and connects an [`IpcPluginConnection`] to it.
async fn connect_mock(
    name: &str,
    capabilities: PluginCapabilities,
) -> (PathBuf, tokio::task::JoinHandle<()>) {
    let socket_path = test_socket_path(name);
    let server = tokio::spawn(run_mock_server(socket_path.clone(), capabilities));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (socket_path, server)
}

/// A plugin's `provides` / `requires` must survive the IPC handshake verbatim
/// and feed the host's capability registry, resolving exactly as the
/// registry unit tests specify.
#[tokio::test]
async fn handshake_declarations_feed_capability_registry() {
    let (socket_path, server) = connect_mock(
        "local-llm",
        PluginCapabilities {
            provides: vec![
                CapabilityRef::parse("llm/chat@1").unwrap(),
                CapabilityRef::parse("embed@1").unwrap(),
                CapabilityRef::parse("gguf-runner@1").unwrap(),
            ],
            requires: vec![CapabilityRequirement::parse("gguf-runner@^1").unwrap()],
            ..PluginCapabilities::default()
        },
    )
    .await;

    let conn = IpcPluginConnection::connect(
        &socket_path,
        ene_plugin_proto::SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake should succeed");

    let advertised = conn.capabilities();
    assert_eq!(advertised.provides.len(), 3);
    assert_eq!(advertised.requires.len(), 1);
    assert_eq!(advertised.provides[2].as_str(), "gguf-runner@1");
    assert_eq!(advertised.requires[0].as_str(), "gguf-runner@^1");

    let mut registry = CapabilityRegistry::new();
    registry.register("local-llm", &advertised);
    assert_eq!(
        registry.resolve(&CapabilityRequirement::parse("gguf-runner@^1").unwrap()),
        Some("local-llm")
    );
    assert!(registry.unmet_hard_requirements("local-llm").is_empty());

    server.abort();
    drop(conn);
    cleanup_path(&socket_path);
}

/// A plugin binary that predates capability declarations omits the fields
/// entirely; the host must see empty declarations and gate nothing.
#[tokio::test]
async fn legacy_handshake_without_declarations_gates_nothing() {
    let (socket_path, server) = connect_mock(
        "legacy",
        PluginCapabilities {
            tools: 1,
            ..PluginCapabilities::default()
        },
    )
    .await;

    let conn = IpcPluginConnection::connect(
        &socket_path,
        ene_plugin_proto::SandboxConfigData::default(),
        None,
        None,
        TEST_HANDSHAKE_TIMEOUT,
        TEST_MAX_CONCURRENT,
    )
    .await
    .expect("handshake should succeed");

    let advertised = conn.capabilities();
    assert!(advertised.provides.is_empty());
    assert!(advertised.requires.is_empty());

    let mut registry = CapabilityRegistry::new();
    registry.register("legacy", &advertised);
    assert!(registry.unmet_hard_requirements("legacy").is_empty());

    server.abort();
    drop(conn);
    cleanup_path(&socket_path);
}
