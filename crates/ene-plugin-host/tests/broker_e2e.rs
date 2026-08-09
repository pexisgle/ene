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

use ene_approval::{ApprovalCategory, ApprovalMode, PluginApprovalPolicy};
use ene_plugin_broker::{BrokerClient, BrokerRequest, BrokerResponse, HttpMethod};
use ene_plugin_host::config::PluginConfig;
use ene_plugin_proto::{
    HostServiceId, HostServiceRequest, HostServiceResponse, SandboxConfigData,
    read_host_service_response, write_host_service_request,
};
use sea_orm::Database;
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
    let server =
        ene_store::host_service::HostServiceServer::new(db, socket.clone(), db_plugins)
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
