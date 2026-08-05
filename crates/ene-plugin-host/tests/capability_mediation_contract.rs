//! Capability-call mediation contract tests over real sockets.
//!
//! A mock provider plugin (raw IPC server) declares `gguf-runner@1` and
//! answers `CapabilityCall` requests; a real `HostServiceServer` +
//! `CapabilityMediator` session mediates calls from a real
//! [`CapabilityClient`] (the consumer side of the wire contract). The
//! consumer's identity comes from the host-service auth token, the ACL from
//! the capability registry built through the startup gate.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests use unwrap/expect for assertions"
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use ene_plugin::CapabilityClient;
use ene_plugin_host::{
    CapabilityCallHandler, CapabilityDeclaration, CapabilityMediator, CapabilityRegistry,
    IpcPluginConnection, ensure_capability_calls_supported, evaluate_capability_gate,
    resolve_capability_provider,
};
use ene_plugin_proto::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityRef,
    CapabilityRequirement, IpcListener, PLUGIN_IPC_MIN_SUPPORTED_VERSION, PluginCapabilities,
    PluginIpcRequest, PluginIpcResponse, VersionRange, WireFormat, cleanup_path,
    read_plugin_request, write_plugin_response,
};
use ene_store::host_service::{DbPluginRegistration, HostServiceServer};
use sea_orm::Database;
use serde_json::{Value, json};

/// Handshake timeout used by the integration tests.
const TEST_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Concurrency bound passed to [`IpcPluginConnection::connect`] in tests.
const TEST_MAX_CONCURRENT: usize = 8;

static SOCKET_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Returns a unique socket path for a test.
fn test_socket_path(name: &str) -> PathBuf {
    let id = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ene-m-{}-{id}-{name}.sock", std::process::id()))
}

fn provider_capabilities(requires: &[&str], supports_calls: bool) -> PluginCapabilities {
    PluginCapabilities {
        provides: vec![CapabilityRef::parse("gguf-runner@1").unwrap()],
        requires: requires
            .iter()
            .map(|raw| CapabilityRequirement::parse(raw).unwrap())
            .collect(),
        supports_capability_calls: supports_calls,
        ..PluginCapabilities::default()
    }
}

fn consumer_capabilities(requires: &[&str]) -> PluginCapabilities {
    PluginCapabilities {
        requires: requires
            .iter()
            .map(|raw| CapabilityRequirement::parse(raw).unwrap())
            .collect(),
        ..PluginCapabilities::default()
    }
}

/// Serves the provider side of the wire contract: handshake (negotiating the
/// oldest protocol version so frames stay JSON), pings, and `CapabilityCall`.
///
/// `method == "fail"` answers `NotSupported`; `method == "crash"` drops the
/// connection mid-session (provider-death simulation).
async fn run_provider_server(socket_path: PathBuf, capabilities: PluginCapabilities) {
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
                min: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
                max: PLUGIN_IPC_MIN_SUPPORTED_VERSION,
            }
            .negotiate(&host_range)
            .unwrap_or(PLUGIN_IPC_MIN_SUPPORTED_VERSION);
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
            while let Ok(Some(request)) = read_plugin_request(&mut stream, WireFormat::Json).await {
                match request {
                    PluginIpcRequest::Ping { request_id } => {
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
                    PluginIpcRequest::CapabilityCall { request_id, call } => {
                        if call.method == "crash" {
                            // Drop the stream: the provider "died".
                            return;
                        }
                        let result = if call.method == "fail" {
                            Err(CapabilityCallError::new(
                                CapabilityCallErrorCode::NotSupported,
                                "mock provider refuses method 'fail'",
                            ))
                        } else {
                            Ok(json!({
                                "echo": {
                                    "capability": call.capability.as_str(),
                                    "method": call.method,
                                    "payload": call.payload,
                                }
                            }))
                        };
                        if write_plugin_response(
                            &mut stream,
                            &PluginIpcResponse::CapabilityCallResult { request_id, result },
                            WireFormat::Json,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        });
    }
}

/// Mediation handler over a registry + connection map, mirroring the runtime
/// handler's resolution (ACL + registry + forward) without a live manager.
struct RegistryHandler {
    registry: CapabilityRegistry,
    connections: HashMap<String, Arc<IpcPluginConnection>>,
}

#[async_trait]
impl CapabilityCallHandler for RegistryHandler {
    async fn call(
        &self,
        consumer: &str,
        call: CapabilityCall,
    ) -> Result<Value, CapabilityCallError> {
        let provider = resolve_capability_provider(&self.registry, consumer, &call)?;
        let connection = self.connections.get(provider).cloned().ok_or_else(|| {
            CapabilityCallError::new(
                CapabilityCallErrorCode::Internal,
                format!("provider plugin {provider} is not connected"),
            )
        })?;
        ensure_capability_calls_supported(&connection.capabilities(), provider)?;
        connection.call_capability(&call).await
    }
}

struct Harness {
    host_service: PathBuf,
    server_task: tokio::task::JoinHandle<()>,
    provider_server: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server_task.abort();
        self.provider_server.abort();
        cleanup_path(&self.host_service);
    }
}

/// Builds the full mediation path: mock provider connection + post-gate
/// registry + real host-service acceptor with the mediator installed.
async fn harness(
    provider_requires: &[&str],
    provider_supports_calls: bool,
    consumers: &[(&str, &[&str])],
) -> Harness {
    let provider_socket = test_socket_path("p");
    let provider_server = tokio::spawn(run_provider_server(
        provider_socket.clone(),
        provider_capabilities(provider_requires, provider_supports_calls),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let connection = Arc::new(
        IpcPluginConnection::connect(
            &provider_socket,
            ene_plugin_proto::SandboxConfigData::default(),
            None,
            None,
            TEST_HANDSHAKE_TIMEOUT,
            TEST_MAX_CONCURRENT,
        )
        .await
        .expect("provider handshake should succeed"),
    );

    let mut declarations = vec![CapabilityDeclaration {
        plugin: "provider-z".to_string(),
        capabilities: connection.capabilities().clone(),
    }];
    declarations.extend(
        consumers
            .iter()
            .map(|(name, requires)| CapabilityDeclaration {
                plugin: (*name).to_string(),
                capabilities: consumer_capabilities(requires),
            }),
    );
    let (registry, _disabled) = evaluate_capability_gate(&declarations);

    let connections = HashMap::from([("provider-z".to_string(), Arc::clone(&connection))]);
    let mediator = CapabilityMediator::with_handler(Arc::new(RegistryHandler {
        registry,
        connections,
    }));

    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory db");
    let host_service = test_socket_path("hs");
    let registrations = consumers
        .iter()
        .map(|(name, _)| {
            (
                format!("ene-db-{name}"),
                DbPluginRegistration {
                    tool_name: (*name).to_string(),
                    prefix: format!("{name}_"),
                    quota_bytes: None,
                },
            )
        })
        .collect();
    let server = HostServiceServer::new(db, host_service.clone(), registrations)
        .with_capability_handler(Arc::new(mediator));
    let server_task = tokio::spawn(async move {
        let _result = server.run().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    Harness {
        host_service,
        server_task,
        provider_server,
    }
}

async fn open_client(harness: &Harness, consumer: &str) -> CapabilityClient {
    CapabilityClient::open(&harness.host_service, &format!("ene-db-{consumer}"))
        .await
        .expect("capability session should open")
}

fn generate_call(payload: Value) -> CapabilityCall {
    CapabilityCall {
        capability: CapabilityRef::parse("gguf-runner@1").unwrap(),
        method: "generate".into(),
        payload,
    }
}

/// The full consumer → host-service → mediator → provider hop: a plugin that
/// declared `requires: ["gguf-runner@^1"]` calls through the host and the
/// provider's echo arrives verbatim.
#[tokio::test]
async fn mediated_call_round_trips_through_host_service() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let result = client
        .call(&generate_call(
            json!({ "model": "stories260K", "prompt": "Once" }),
        ))
        .await
        .expect("mediated call should succeed");
    assert_eq!(
        result,
        json!({
            "echo": {
                "capability": "gguf-runner@1",
                "method": "generate",
                "payload": { "model": "stories260K", "prompt": "Once" },
            }
        })
    );
}

/// A consumer with no capability declarations at all is forbidden.
#[tokio::test]
async fn caller_without_requirement_is_forbidden() {
    let harness = harness(&[], true, &[("consumer-a", &[])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&generate_call(json!({})))
        .await
        .expect_err("undocumented call must be forbidden");
    assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
}

/// A requirement for a different capability does not authorize the call.
#[tokio::test]
async fn mismatched_requirement_is_forbidden() {
    let harness = harness(&[], true, &[("consumer-a", &["embed@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&generate_call(json!({})))
        .await
        .expect_err("mismatched requirement must be forbidden");
    assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
}

/// Soft requirements authorize calls (they are still declarations of intent).
#[tokio::test]
async fn soft_requirement_authorizes() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1?"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let result = client
        .call(&generate_call(json!({ "model": "m" })))
        .await
        .expect("soft requirement should authorize");
    assert_eq!(result["echo"]["method"], "generate");
}

/// A malformed capability reference is a request error, not a provider error.
#[tokio::test]
async fn malformed_capability_is_invalid_request() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let malformed = CapabilityCall {
        capability: serde_json::from_value(json!("not a capability")).unwrap(),
        ..generate_call(json!({}))
    };
    let error = client.call(&malformed).await.expect_err("malformed ref");
    assert_eq!(error.code, CapabilityCallErrorCode::InvalidRequest);
}

/// The provider's typed rejection propagates through the host unchanged.
#[tokio::test]
async fn provider_rejection_propagates_unchanged() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&CapabilityCall {
            method: "fail".into(),
            ..generate_call(json!({}))
        })
        .await
        .expect_err("provider rejection should propagate");
    assert_eq!(error.code, CapabilityCallErrorCode::NotSupported);
    assert_eq!(error.message, "mock provider refuses method 'fail'");
}

/// A provider binary that predates capability calls (declares the capability
/// but not `supports_capability_calls`) is refused with a typed
/// `not_supported` — never a connection-level decode failure — and the
/// consumer session survives for a subsequent call.
#[tokio::test]
async fn predating_provider_yields_typed_not_supported() {
    let harness = harness(&[], false, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&generate_call(json!({})))
        .await
        .expect_err("predating provider must be refused");
    assert_eq!(error.code, CapabilityCallErrorCode::NotSupported);
    assert!(
        error.message.contains("predates"),
        "refusal must name the N-1 cause: {}",
        error.message
    );

    let again = client
        .call(&generate_call(json!({})))
        .await
        .expect_err("session survives; still refused");
    assert_eq!(again.code, CapabilityCallErrorCode::NotSupported);
}

/// A provider disabled by the startup gate never satisfies a mediated call:
/// the post-gate registry has no provider left, so the consumer gets
/// `NoProvider` even though its requirement is declared.
#[tokio::test]
async fn gate_disabled_provider_yields_no_provider() {
    let harness = harness(&["missing@1"], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&generate_call(json!({})))
        .await
        .expect_err("disabled provider must not satisfy calls");
    assert_eq!(error.code, CapabilityCallErrorCode::NoProvider);
}

/// A provider that dies mid-call surfaces `Transport` to the consumer, and
/// the consumer's session survives: the next call round-trips again through
/// the connection's reconnect machinery.
#[tokio::test]
async fn provider_crash_propagates_transport_and_session_survives() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let mut client = open_client(&harness, "consumer-a").await;
    let error = client
        .call(&CapabilityCall {
            method: "crash".into(),
            ..generate_call(json!({}))
        })
        .await
        .expect_err("provider death must surface as transport failure");
    assert_eq!(error.code, CapabilityCallErrorCode::Transport);

    let result = client
        .call(&generate_call(json!({ "model": "m" })))
        .await
        .expect("consumer session must survive a provider crash");
    assert_eq!(result["echo"]["method"], "generate");
}

/// An unknown token is rejected when opening the capability session.
#[tokio::test]
async fn unknown_token_is_rejected_at_open() {
    let harness = harness(&[], true, &[("consumer-a", &["gguf-runner@^1"])]).await;
    let Err(error) = CapabilityClient::open(&harness.host_service, "ene-db-impostor").await else {
        panic!("unknown token must be rejected");
    };
    match error {
        ene_plugin::CapabilityClientError::Open { code, .. } => {
            assert_eq!(code, ene_plugin_proto::HostServiceErrorCode::AuthRejected);
        }
        other => panic!("unexpected error: {other}"),
    }
}

/// A host-service acceptor without a capability handler keeps rejecting the
/// service with `UnknownService` (the pre-mediation behavior).
#[tokio::test]
async fn capability_open_without_handler_is_unknown_service() {
    let host_service = test_socket_path("nh");
    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory db");
    let registrations = HashMap::from([(
        "ene-db-consumer-a".to_string(),
        DbPluginRegistration {
            tool_name: "consumer-a".into(),
            prefix: "consumer-a_".into(),
            quota_bytes: None,
        },
    )]);
    let server = HostServiceServer::new(db, host_service.clone(), registrations);
    let server_task = tokio::spawn(async move {
        let _result = server.run().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let Err(error) = CapabilityClient::open(&host_service, "ene-db-consumer-a").await else {
        panic!("capability service must be unimplemented without a handler");
    };
    match error {
        ene_plugin::CapabilityClientError::Open { code, .. } => {
            assert_eq!(code, ene_plugin_proto::HostServiceErrorCode::UnknownService);
        }
        other => panic!("unexpected error: {other}"),
    }
    server_task.abort();
    cleanup_path(&host_service);
}
