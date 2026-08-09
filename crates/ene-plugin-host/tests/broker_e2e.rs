//! End-to-end broker-channel tests through the real host-service server.
//!
//! A [`BrokerClient`] connects to a real [`HostServiceServer`] backed by a
//! [`BrokerHub`] built from plugin configuration, proving the production
//! gate order: identity binding, manifest layer, approval layer, and
//! mandatory constraints (SSRF).

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "integration test uses expect/panic for concise assertions"
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use ed25519_dalek::Signer as _;
use ene_approval::{ApprovalCategory, ApprovalMode, PluginApprovalPolicy};
use ene_approval::{
    ManifestPermission, ManifestSideEffects, PluginManifest, ResourceLimits, SignedManifest,
};
use ene_plugin_broker::{BrokerClient, BrokerRequest, BrokerResponse, HttpMethod};
use ene_plugin_host::config::PluginConfig;
use ene_plugin_proto::{
    HostServiceId, HostServiceRequest, HostServiceResponse, SandboxConfigData,
    read_host_service_response, write_host_service_request,
};
use sea_orm::Database;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

/// Allows every interactive request (used to prove that mandatory
/// constraints still hold when the approval layer says yes).
struct AllowAllResponder;

#[async_trait::async_trait]
impl ene_plugin_host::ApprovalResponder for AllowAllResponder {
    async fn request(
        &self,
        _plugin: &str,
        _category: ApprovalCategory,
        _target: &str,
    ) -> ene_approval::ResolvedMode {
        ene_approval::ResolvedMode::Allow
    }
}

fn test_config() -> PluginConfig {
    let mut config = PluginConfig::default();
    // Built-ins only: `web` declares `network` + `file` services with
    // `dynamic_web`; `utility` declares the `platform` service.
    config
        .list
        .retain(|name, _| name == "web" || name == "utility");
    config
}

fn test_config_deny_dynamic_https() -> PluginConfig {
    let mut config = test_config();
    let mut plugin_policy = PluginApprovalPolicy::default();
    plugin_policy
        .categories
        .insert(ApprovalCategory::DynamicHttps, ApprovalMode::Deny);
    config.plugin_approval = BTreeMap::from([("web".to_string(), plugin_policy)]);
    config
}

fn test_config_with_fs_grant(root: &std::path::Path) -> PluginConfig {
    let mut config = test_config();
    // The `fs` built-in manifest declares a `workspace` slot with
    // read+write permissions; bind it to a temp directory.
    config.list.insert(
        "fs".to_string(),
        ene_plugin_host::PluginEntry {
            enable: true,
            fs_grants: vec![ene_plugin_host::config::FsGrantConfig {
                slot: "workspace".to_string(),
                path: root.to_string_lossy().into_owned(),
                read: true,
                write: true,
            }],
            ..Default::default()
        },
    );
    config
}

fn test_signing_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
}

fn test_artifact_manifest() -> SignedManifest {
    let manifest = PluginManifest {
        schema_version: 1,
        plugin_id: "artifact-test".to_string(),
        name: "Artifact Test".to_string(),
        publisher: "test-publisher".to_string(),
        version: "1".to_string(),
        description: None,
        fs_slots: vec![],
        fixed_origins: vec![],
        dynamic_web: false,
        artifacts: vec![],
        sidecars: vec![],
        host_services: vec!["artifact".to_string()],
        side_effects: ManifestSideEffects::default(),
        resource_limits: ResourceLimits::default(),
        permissions: vec![ManifestPermission {
            category: ApprovalCategory::ModelInstall,
            max: ApprovalMode::Allow,
        }],
    };
    let payload = ene_approval::canonical_manifest_bytes(&manifest).expect("canonical");
    let key = test_signing_key();
    SignedManifest {
        signature: Some(key.sign(&payload).to_bytes().to_vec()),
        key_id: Some("test-publisher".to_string()),
        payload,
    }
}

fn signed_catalog_with_expiry(
    version: u64,
    artifact_version: &str,
    expires_at_ms: u64,
) -> ene_artifact::SignedCatalog {
    let metadata = ene_artifact::CatalogMetadata {
        version,
        expires_at_ms,
        artifacts: std::collections::BTreeMap::from([(
            "fs".to_string(),
            ene_artifact::ArtifactTarget {
                version: artifact_version.to_string(),
                kind: ene_artifact::ArtifactKind::Plugin,
                urls: vec!["https://example.test/fs.bin".to_string()],
                sha256: "ab".repeat(32),
                size: 4,
            },
        )]),
    };
    let key = test_signing_key();
    ene_artifact::sign_catalog(&metadata, "test-publisher".to_string(), &key).expect("sign")
}

/// Serves the current signed catalog JSON over plain HTTP on loopback.
async fn serve_catalog(
    catalog: std::sync::Arc<parking_lot::Mutex<ene_artifact::SignedCatalog>>,
) -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let catalog = std::sync::Arc::clone(&catalog);
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                drop(socket.read(&mut buf).await);
                let body = serde_json::to_vec(&*catalog.lock()).expect("serialize catalog");
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                drop(socket.write_all(head.as_bytes()).await);
                drop(socket.write_all(&body).await);
                drop(socket.shutdown().await);
            });
        }
    });
    (server, format!("http://{addr}/catalog.json"))
}

async fn spawn_host_service(
    socket: &std::path::Path,
    config: &PluginConfig,
) -> tokio::task::JoinHandle<()> {
    let db = Database::connect("sqlite::memory:").await.expect("db");
    let hub = ene_plugin_host::BrokerHub::from_config(config).expect("hub");
    let hub = hub.with_approval_responder(Arc::new(AllowAllResponder));
    let db_plugins = std::collections::HashMap::from([
        (
            "web-token".to_string(),
            ene_store::host_service::DbPluginRegistration {
                tool_name: "web".to_string(),
                prefix: "web_".to_string(),
                quota_bytes: None,
            },
        ),
        (
            "utility-token".to_string(),
            ene_store::host_service::DbPluginRegistration {
                tool_name: "utility".to_string(),
                prefix: "utility_".to_string(),
                quota_bytes: None,
            },
        ),
    ]);
    let server =
        ene_store::host_service::HostServiceServer::new(db, socket.to_path_buf(), db_plugins)
            .with_broker_handler(hub);
    tokio::spawn(async move {
        if let Err(e) = server.run().await {
            panic!("host service server failed: {e}");
        }
    })
}

async fn wait_for_socket(path: &std::path::Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        tokio::task::yield_now().await;
    }
}

async fn open_network_session(socket: &std::path::Path) -> BrokerClient {
    BrokerClient::connect(socket, "web-token", HostServiceId::Network)
        .await
        .expect("open network session")
}

async fn open_file_session(socket: &std::path::Path, token: &str) -> BrokerClient {
    BrokerClient::connect(socket, token, HostServiceId::File)
        .await
        .expect("open file session")
}

#[tokio::test]
async fn ssrf_blocks_loopback_even_when_the_approval_says_allow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service.sock");
    let config = test_config();
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    let mut client = open_network_session(&socket).await;
    let response = client
        .request(&BrokerRequest::NetworkFetch {
            method: HttpMethod::Get,
            url: "https://127.0.0.1:1/secret".to_string(),
            headers: vec![],
            body: None,
            max_bytes: None,
        })
        .await;
    server.abort();

    let err = response.expect_err("loopback must be denied");
    let message = format!("{err}");
    assert!(
        message.to_lowercase().contains("ssrf"),
        "denial must name the SSRF guard: {message}"
    );
}

#[tokio::test]
async fn denied_category_is_rejected_before_any_network_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-deny.sock");
    let mut config = test_config();
    let mut plugin_policy = PluginApprovalPolicy::default();
    plugin_policy
        .categories
        .insert(ApprovalCategory::DynamicHttps, ApprovalMode::Deny);
    config.plugin_approval = BTreeMap::from([("web".to_string(), plugin_policy)]);
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    let mut client = open_network_session(&socket).await;
    let response = client
        .request(&BrokerRequest::NetworkFetch {
            method: HttpMethod::Get,
            url: "https://example.com/".to_string(),
            headers: vec![],
            body: None,
            max_bytes: None,
        })
        .await;
    server.abort();

    let err = response.expect_err("policy denial must win");
    assert!(
        format!("{err}").contains("denied by policy"),
        "denial must cite the policy: {err}"
    );
}

#[tokio::test]
async fn streaming_requests_route_through_the_session_loop_and_apply_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-stream.sock");
    let config = test_config_deny_dynamic_https();
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    let mut client = open_network_session(&socket).await;
    let response = client
        .collect_stream(&BrokerRequest::NetworkFetchStream {
            method: HttpMethod::Get,
            url: "https://example.com/stream".to_string(),
            headers: vec![],
            body: None,
            max_bytes: None,
        })
        .await;
    server.abort();

    let err = response.expect_err("policy denial must reach the stream client");
    assert!(
        format!("{err}").contains("denied by policy"),
        "streaming denial must cite the policy: {err}"
    );
}

#[tokio::test]
async fn undeclared_capabilities_are_rejected_even_with_allow_all() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-undeclared.sock");
    let config = test_config();
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    // The web manifest declares no CredentialUse permission.
    let mut client = open_network_session(&socket).await;
    let response = client
        .request(&BrokerRequest::CredentialGet {
            key: "TAVILY_API_KEY".to_string(),
        })
        .await;
    server.abort();

    let err = response.expect_err("credential access must be rejected");
    assert!(
        format!("{err}").contains("not declared"),
        "denial must cite the manifest layer: {err}"
    );
}

#[tokio::test]
async fn declared_platform_service_is_served() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-platform.sock");
    let config = test_config();
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    let mut client = open_network_session(&socket).await;
    // The manifest declares the `platform` service, but this session opened
    // `network`; opening a second session for `platform` must work.
    let mut platform = BrokerClient::connect(&socket, "utility-token", HostServiceId::Platform)
        .await
        .expect("open platform session");
    let response = platform
        .request(&BrokerRequest::PlatformNow)
        .await
        .expect("platform now");
    assert!(matches!(response, BrokerResponse::PlatformNowOk { unix_ms } if unix_ms > 0));

    // The `network` session still serves requests (identity is pinned per
    // token, not per session).
    let response = client
        .request(&BrokerRequest::NetworkFetch {
            method: HttpMethod::Get,
            url: "https://10.0.0.1/private".to_string(),
            headers: vec![],
            body: None,
            max_bytes: None,
        })
        .await
        .expect_err("private address must be denied");
    assert!(
        format!("{response}").to_lowercase().contains("ssrf"),
        "denial must name the SSRF guard"
    );
    drop(platform);
    server.abort();
}

#[tokio::test]
async fn unknown_token_is_rejected_at_open() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-auth.sock");
    let config = test_config();
    let server = spawn_host_service(&socket, &config).await;
    wait_for_socket(&socket).await;

    let mut stream = ene_plugin_proto::IpcStream::connect(&socket)
        .await
        .expect("connect");
    write_host_service_request(
        &mut stream,
        &HostServiceRequest::Open {
            service: HostServiceId::Network,
            token: "forged-token".to_string(),
        },
    )
    .await
    .expect("write open");
    let response = read_host_service_response(&mut stream)
        .await
        .expect("read open response");
    drop(stream.shutdown().await);
    server.abort();

    assert!(matches!(
        response,
        Some(HostServiceResponse::Error {
            code: ene_plugin_proto::HostServiceErrorCode::AuthRejected,
            ..
        })
    ));
}

/// Ensures the sandbox payload the host would send still deserializes with
/// the new broker fields (schema regression guard).
#[test]
fn sandbox_config_with_broker_fields_round_trips() {
    let config = SandboxConfigData {
        broker_socket: Some("/tmp/ene-host-service.sock".to_string()),
        plugin_temp_dir: Some("/tmp/ene-plugin-tmp".to_string()),
        db_auth_token: Some("tok".to_string()),
        ..Default::default()
    };
    let json = serde_json::to_string(&config).expect("serialize");
    let back: SandboxConfigData = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        back.broker_socket.as_deref(),
        Some("/tmp/ene-host-service.sock")
    );
    assert_eq!(back.plugin_temp_dir.as_deref(), Some("/tmp/ene-plugin-tmp"));
}

#[tokio::test]
async fn signed_catalog_refreshes_on_demand_and_rejects_rollback() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("host-service-artifact.sock");

    let current = std::sync::Arc::new(parking_lot::Mutex::new(signed_catalog_with_expiry(
        1,
        "1.0.0",
        u64::MAX,
    )));
    let (catalog_server, catalog_url) = serve_catalog(std::sync::Arc::clone(&current)).await;

    let manifest = test_artifact_manifest();
    let mut config = PluginConfig::default();
    config.list.clear();
    config.list.insert(
        "artifact-test".to_string(),
        ene_plugin_host::PluginEntry {
            enable: true,
            manifest: Some(manifest),
            ..Default::default()
        },
    );
    config.trusted_publishers = vec![ene_plugin_host::config::TrustedPublisherConfig {
        publisher: "test-publisher".to_string(),
        public_key_hex: hex::encode(test_signing_key().verifying_key().to_bytes()),
    }];
    config.artifact = ene_plugin_host::config::ArtifactConfig {
        enabled: true,
        catalog_url: Some(catalog_url),
        catalog_keys: vec![ene_plugin_host::config::CatalogKeyConfig {
            key_id: "test-publisher".to_string(),
            public_key_hex: hex::encode(test_signing_key().verifying_key().to_bytes()),
        }],
        refresh_hours: 1,
        ..Default::default()
    };

    let db = Database::connect("sqlite::memory:").await.expect("db");
    let hub = ene_plugin_host::BrokerHub::from_config(&config).expect("hub");
    let hub = hub.with_approval_responder(Arc::new(AllowAllResponder));
    let db_plugins = std::collections::HashMap::from([(
        "artifact-token".to_string(),
        ene_store::host_service::DbPluginRegistration {
            tool_name: "artifact-test".to_string(),
            prefix: "artifact_test_".to_string(),
            quota_bytes: None,
        },
    )]);
    let server = ene_store::host_service::HostServiceServer::new(db, socket.clone(), db_plugins)
        .with_broker_handler(hub);
    let server = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            panic!("host service server failed: {e}");
        }
    });
    wait_for_socket(&socket).await;

    let mut artifact = BrokerClient::connect(&socket, "artifact-token", HostServiceId::Artifact)
        .await
        .expect("open artifact session");

    // First resolve fetches + verifies the signed catalog.
    let response = artifact
        .request(&BrokerRequest::ArtifactResolve {
            artifact_id: "fs".to_string(),
            version: None,
        })
        .await
        .expect("resolve");
    match response {
        BrokerResponse::ArtifactResolveOk { artifact } => {
            assert_eq!(artifact.version, "1.0.0");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    // The catalog is updated server-side; within the refresh window the
    // cached metadata still serves the old version.
    *current.lock() = signed_catalog_with_expiry(2, "1.0.1", u64::MAX);
    let response = artifact
        .request(&BrokerRequest::ArtifactResolve {
            artifact_id: "fs".to_string(),
            version: None,
        })
        .await
        .expect("cached resolve");
    match response {
        BrokerResponse::ArtifactResolveOk { artifact } => {
            assert_eq!(
                artifact.version, "1.0.0",
                "cache must serve the old catalog"
            );
        }
        other => panic!("unexpected response: {other:?}"),
    }

    // Manual refresh re-fetches and re-verifies.
    let response = artifact
        .request(&BrokerRequest::ArtifactRefresh)
        .await
        .expect("refresh");
    assert!(matches!(
        response,
        BrokerResponse::ArtifactRefreshOk { catalog_version: 2 }
    ));
    let response = artifact
        .request(&BrokerRequest::ArtifactResolve {
            artifact_id: "fs".to_string(),
            version: None,
        })
        .await
        .expect("resolve after refresh");
    match response {
        BrokerResponse::ArtifactResolveOk { artifact } => {
            assert_eq!(artifact.version, "1.0.1");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    // An expired catalog is rejected by the verifier even on forced
    // refresh (rollback/digest-change rejection against the installed state
    // is covered by the ene-artifact verifier unit tests).
    *current.lock() = signed_catalog_with_expiry(3, "1.0.2", 1);
    let response = artifact
        .request(&BrokerRequest::ArtifactRefresh)
        .await
        .expect_err("expired refresh must be denied");
    assert!(
        format!("{response}").contains("expired"),
        "denial must cite the expiry guard: {response}"
    );
    // The previously verified catalog stays cached.
    let response = artifact
        .request(&BrokerRequest::ArtifactResolve {
            artifact_id: "fs".to_string(),
            version: None,
        })
        .await
        .expect("resolve after rejected refresh");
    match response {
        BrokerResponse::ArtifactResolveOk { artifact } => {
            assert_eq!(artifact.version, "1.0.1");
        }
        other => panic!("unexpected response: {other:?}"),
    }

    drop(artifact);
    server.abort();
    catalog_server.abort();
}

#[tokio::test]
async fn file_broker_serves_granted_absolute_paths_and_denies_others() {
    let dir = tempfile::tempdir().expect("tempdir");
    let grant = dir.path().join("granted");
    std::fs::create_dir_all(&grant).expect("mkdir");
    let outside = dir.path().join("outside.txt");
    std::fs::write(&outside, b"secret").expect("write");

    let socket = dir.path().join("host-service-file.sock");
    let config = test_config_with_fs_grant(&grant);
    let db = Database::connect("sqlite::memory:").await.expect("db");
    let hub = ene_plugin_host::BrokerHub::from_config(&config).expect("hub");
    let hub = hub.with_approval_responder(Arc::new(AllowAllResponder));
    let db_plugins = std::collections::HashMap::from([(
        "fs-token".to_string(),
        ene_store::host_service::DbPluginRegistration {
            tool_name: "fs".to_string(),
            prefix: "fs_".to_string(),
            quota_bytes: None,
        },
    )]);
    let server = ene_store::host_service::HostServiceServer::new(db, socket.clone(), db_plugins)
        .with_broker_handler(hub);
    let server = tokio::spawn(async move {
        if let Err(e) = server.run().await {
            panic!("host service server failed: {e}");
        }
    });
    wait_for_socket(&socket).await;

    let mut file = open_file_session(&socket, "fs-token").await;
    let target = grant.join("notes.txt");
    let target_str = target.to_string_lossy().into_owned();
    let response = file
        .request(&BrokerRequest::FileWrite {
            path: target_str.clone(),
            data: b"hello broker".to_vec(),
            create: true,
            truncate: true,
        })
        .await
        .expect("write inside grant");
    assert!(matches!(response, BrokerResponse::FileWriteOk { .. }));

    let response = file
        .request(&BrokerRequest::FileRead {
            path: target_str.clone(),
            max_bytes: Some(1024),
        })
        .await
        .expect("read inside grant");
    match response {
        BrokerResponse::FileReadOk { data, size, .. } => {
            assert_eq!(data, b"hello broker");
            assert_eq!(size, 12);
        }
        other => panic!("unexpected response: {other:?}"),
    }

    // Absolute path outside the grant is denied even with an allow-all
    // responder (mandatory grant containment).
    let response = file
        .request(&BrokerRequest::FileRead {
            path: outside.to_string_lossy().into_owned(),
            max_bytes: Some(1024),
        })
        .await
        .expect_err("outside grant must be denied");
    assert!(
        format!("{response}").contains("grant"),
        "denial must cite the grant: {response}"
    );

    // Directory listing + recursive delete through the broker.
    let sub = grant.join("sub").join("deep");
    file.request(&BrokerRequest::FileCreateDir {
        path: sub.to_string_lossy().into_owned(),
        recursive: true,
    })
    .await
    .expect("create dir");
    let response = file
        .request(&BrokerRequest::FileList {
            path: grant.to_string_lossy().into_owned(),
        })
        .await
        .expect("list");
    assert!(
        matches!(
            response,
            BrokerResponse::FileListOk { entries } if entries.iter().any(|e| e.name == "sub")
        ),
        "list must include the created subdirectory"
    );
    file.request(&BrokerRequest::FileDelete {
        path: sub.to_string_lossy().into_owned(),
        recursive: true,
    })
    .await
    .expect("recursive delete");
    assert!(!sub.exists());

    drop(file);
    server.abort();
}
