//! End-to-end gguf-runner mediation contract against the real plugin binary.
//!
//! Spawns `ene-plugin-llama-cpp` (the `gguf-runner@1` provider), connects it
//! through the host-side `IpcPluginConnection`, and drives the published
//! runner methods (`generate` / `embed` / `unload`) from a consumer-side
//! `CapabilityClient` through a real host-service acceptor + capability
//! mediator — the exact path a third-party consumer plugin takes. The GGUF
//! fixtures follow the same pinned + skip-on-unavailable policy as
//! `inference_contract.rs` (harness in `common`).
#![expect(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::unwrap_used,
    reason = "integration tests use expect/panic for assertions and eprintln for skip diagnostics"
)]

mod common;
use common::*;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ene_plugin::CapabilityClient;
use ene_plugin_host::{
    CapabilityCallHandler, CapabilityDeclaration, CapabilityMediator, CapabilityRegistry,
    IpcPluginConnection, evaluate_capability_gate, resolve_capability_provider,
};
use ene_plugin_proto::{
    CapabilityCall, CapabilityCallError, CapabilityCallErrorCode, CapabilityRef,
    CapabilityRequirement, PluginCapabilities, SandboxConfigData, cleanup_path,
};
use ene_store::host_service::{DbPluginRegistration, HostServiceServer};
use sea_orm::Database;
use serde_json::{Value, json};

const TEST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_MAX_CONCURRENT: usize = 8;

/// Mediation handler over a registry + connection map (mirrors the runtime
/// handler's resolution; the manager itself is covered by the host crate's
/// mediation suite).
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
        connection.call_capability(&call).await
    }
}

/// The full consumer → host-service → mediator → real provider path.
struct MediationHarness {
    host_service: PathBuf,
    server_task: tokio::task::JoinHandle<()>,
    _connection: Arc<IpcPluginConnection>,
    _child: ChildGuard,
}

impl Drop for MediationHarness {
    fn drop(&mut self) {
        self.server_task.abort();
        cleanup_path(&self.host_service);
    }
}

async fn harness(fixtures: &Fixtures) -> MediationHarness {
    let socket_path = test_socket_path("provider");
    cleanup_path(&socket_path);
    let child = Command::new(plugin_binary())
        .env("ENE_PLUGIN_SOCKET", &socket_path)
        .spawn()
        .expect("spawn plugin binary");

    let profiles = json!({
        "chat-fixture": {
            "model_path": fixtures.chat.to_str().expect("utf8 fixture path"),
            "quantization": "F16",
            "gpu_layers": "0",
            "context_size": 2048,
        },
        "embed-fixture": {
            "model_path": fixtures.embed.to_str().expect("utf8 fixture path"),
            "quantization": "F16",
            "gpu_layers": "0",
        }
    });
    let connection = Arc::new(
        IpcPluginConnection::connect(
            &socket_path,
            SandboxConfigData::default(),
            Some(json!({ "acceleration": "cpu" })),
            Some(profiles),
            TEST_HANDSHAKE_TIMEOUT,
            TEST_MAX_CONCURRENT,
        )
        .await
        .expect("provider handshake should succeed"),
    );

    let declarations = vec![
        CapabilityDeclaration {
            plugin: "local-llm".to_string(),
            capabilities: connection.capabilities().clone(),
        },
        CapabilityDeclaration {
            plugin: "consumer".to_string(),
            capabilities: PluginCapabilities {
                requires: vec![CapabilityRequirement::parse("gguf-runner@^1").unwrap()],
                ..PluginCapabilities::default()
            },
        },
    ];
    let (registry, _disabled) = evaluate_capability_gate(&declarations);
    let connections = HashMap::from([("local-llm".to_string(), Arc::clone(&connection))]);
    let mediator = CapabilityMediator::with_handler(Arc::new(RegistryHandler {
        registry,
        connections,
    }));

    let db = Database::connect("sqlite::memory:")
        .await
        .expect("in-memory db");
    let host_service = test_socket_path("hs");
    let registrations = HashMap::from([
        (
            "ene-db-consumer".to_string(),
            DbPluginRegistration {
                tool_name: "consumer".into(),
                prefix: "consumer_".into(),
                quota_bytes: None,
            },
        ),
        (
            "ene-db-sneaky".to_string(),
            DbPluginRegistration {
                tool_name: "sneaky".into(),
                prefix: "sneaky_".into(),
                quota_bytes: None,
            },
        ),
    ]);
    let server = HostServiceServer::new(db, host_service.clone(), registrations)
        .with_capability_handler(Arc::new(mediator));
    let server_task = tokio::spawn(async move {
        let _result = server.run().await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    MediationHarness {
        host_service,
        server_task,
        _connection: connection,
        _child: ChildGuard::new(child),
    }
}

fn runner_call(method: &str, payload: Value) -> CapabilityCall {
    CapabilityCall {
        capability: CapabilityRef::parse("gguf-runner@1").unwrap(),
        method: method.to_string(),
        payload,
    }
}

/// The published `gguf-runner@1` methods work end-to-end: generate (plain and
/// JSON-schema-constrained), embed, and unload followed by a successful
/// reload — all through the host-mediated path.
#[tokio::test]
async fn gguf_runner_mediation_serves_real_inference() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let harness = harness(&fixtures).await;
    let mut client = CapabilityClient::open(&harness.host_service, "ene-db-consumer")
        .await
        .expect("capability session should open");

    let generated = client
        .call(&runner_call(
            "generate",
            json!({ "model": "chat-fixture", "prompt": "Once upon a time" }),
        ))
        .await
        .expect("generate should succeed");
    let text = generated["text"]
        .as_str()
        .expect("generate returns { text }");
    assert!(!text.trim().is_empty(), "expected non-empty generated text");

    let structured = client
        .call(&runner_call(
            "generate",
            json!({
                "model": "chat-fixture",
                "prompt": "Reply with JSON only: {\"ok\": true}",
                "json_schema": {
                    "type": "object",
                    "properties": { "ok": { "type": "boolean" } },
                    "required": ["ok"],
                    "additionalProperties": false
                }
            }),
        ))
        .await
        .expect("schema-constrained generate should succeed");
    assert!(
        !structured["text"].as_str().unwrap_or("").trim().is_empty(),
        "expected non-empty structured output"
    );

    let embedded = client
        .call(&runner_call(
            "embed",
            json!({
                "model": "embed-fixture",
                "texts": [
                    "The cat sat on the mat.",
                    "A dog barked at the moon."
                ]
            }),
        ))
        .await
        .expect("embed should succeed");
    let embeddings = embedded["embeddings"]
        .as_array()
        .expect("embed returns { embeddings }");
    assert_eq!(embeddings.len(), 2, "one vector per input item");
    for vector in embeddings {
        let dims = vector.as_array().expect("each embedding is an array");
        assert_eq!(dims.len(), 384, "bge-small produces 384 dims");
        assert!(
            dims.iter()
                .all(|value| value.as_f64().is_some_and(f64::is_finite)),
            "embedding values must be finite"
        );
    }

    let unloaded = client
        .call(&runner_call("unload", json!({ "model": "chat-fixture" })))
        .await
        .expect("unload should succeed");
    assert_eq!(unloaded, json!({ "ok": true }));

    // Unload released the resident chat model; the next call reloads it.
    let reloaded = client
        .call(&runner_call(
            "generate",
            json!({ "model": "chat-fixture", "prompt": "Once more" }),
        ))
        .await
        .expect("generate after unload should reload and succeed");
    assert!(
        !reloaded["text"].as_str().unwrap_or("").trim().is_empty(),
        "expected non-empty text after reload"
    );
}

/// Unknown models surface as typed provider errors and the plugin process
/// survives (liveness through the mediated path).
#[tokio::test]
async fn mediation_unknown_model_returns_typed_error_and_plugin_survives() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let harness = harness(&fixtures).await;
    let mut client = CapabilityClient::open(&harness.host_service, "ene-db-consumer")
        .await
        .expect("capability session should open");

    let error = client
        .call(&runner_call(
            "generate",
            json!({ "model": "no-such-model", "prompt": "hi" }),
        ))
        .await
        .expect_err("unknown model must fail");
    assert_eq!(error.code, CapabilityCallErrorCode::Provider);
    assert!(
        error.message.contains("profile"),
        "missing profile error: {}",
        error.message
    );

    let recovered = client
        .call(&runner_call(
            "generate",
            json!({ "model": "chat-fixture", "prompt": "Say hello." }),
        ))
        .await
        .expect("plugin must survive the typed error");
    assert!(
        !recovered["text"].as_str().unwrap_or("").trim().is_empty(),
        "expected non-empty text after recovery"
    );
}

/// The ACL applies on the real path: a consumer that declared no `requires`
/// cannot call the provider.
#[tokio::test]
async fn mediation_forbids_undeclared_consumer() {
    let Some(fixtures) = Fixtures::fetch().await else {
        return;
    };
    let harness = harness(&fixtures).await;
    // "sneaky" has a valid auth token but no capability declarations in the
    // registry, so its calls must be forbidden.
    let mut client = CapabilityClient::open(&harness.host_service, "ene-db-sneaky")
        .await
        .expect("capability session should open");

    let error = client
        .call(&runner_call(
            "generate",
            json!({ "model": "chat-fixture", "prompt": "hi" }),
        ))
        .await
        .expect_err("undeclared consumer must be forbidden");
    assert_eq!(error.code, CapabilityCallErrorCode::Forbidden);
}
