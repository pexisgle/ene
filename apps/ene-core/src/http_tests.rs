use crate::{BootOptions, CoreDaemon};
use async_trait::async_trait;
use ene_api::{
    ApiClient, ClaimResourceRequest, CreateSessionRequest, MessageMode, MessageRequest,
    ResourceKind,
};
use ene_kernel::{ConversationModel, EchoModel, KernelError, ModelGeneration, ModelRequest};
use ene_plane::{ApprovalMode, AuthzRequest, Sensitivity};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

async fn boot_server() -> (TempDir, ApiClient, Arc<CoreDaemon>, crate::ServerHandle) {
    let dir = TempDir::new().unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    let server = core
        .clone()
        .serve_at(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(EchoModel) as Arc<dyn ConversationModel>,
        )
        .await
        .unwrap();
    let token = std::fs::read_to_string(core.data_dir().join("api.token")).unwrap();
    let base = format!("http://{}", server.addr);
    let client = ApiClient::new(base, token.trim(), "stage");
    (dir, client, core, server)
}

#[tokio::test]
async fn three_clients_share_one_core() {
    let (_dir, stage, _core, server) = boot_server().await;
    let cli = ApiClient::new(stage.base(), stage.token(), "cli");
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    assert_eq!(stage.health().await.unwrap().status, "ok");
    assert_eq!(cli.health().await.unwrap().status, "ok");
    assert_eq!(web.health().await.unwrap().status, "ok");
    let openapi = stage.openapi().await.unwrap();
    assert_eq!(openapi["info"]["title"], "ene-core API");
    server.shutdown().await;
}

#[tokio::test]
async fn surface_ws_never_sees_inner() {
    let (_dir, client, _core, server) = boot_server().await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    let mut surface = client.events("surface", Some(&session.id)).await.unwrap();
    let mut detail = client.events("detail", Some(&session.id)).await.unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "hello inner".into(),
                mode: MessageMode::Prompt,
            },
            None,
        )
        .await
        .unwrap();
    let mut saw_surface_text = false;
    let mut saw_detail_inner = false;
    let mut surface_leaked_inner = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !(saw_surface_text && saw_detail_inner) {
        tokio::select! {
            ev = surface.recv_json() => {
                if let Ok(Some(value)) = ev {
                    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if kind == "inner.message" || kind == "thinking.delta" {
                        surface_leaked_inner = true;
                    }
                    if kind == "text.delta" {
                        saw_surface_text = true;
                    }
                }
            }
            ev = detail.recv_json() => {
                if let Ok(Some(value)) = ev
                    && value.get("type").and_then(|v| v.as_str()) == Some("inner.message")
                {
                    saw_detail_inner = true;
                }
            }
        }
    }
    assert!(saw_surface_text, "surface should receive speech");
    assert!(saw_detail_inner, "detail should receive inner");
    assert!(!surface_leaked_inner, "surface must not receive inner");
    let surface_hist = client.history(&session.id, "surface").await.unwrap();
    assert!(surface_hist.messages.iter().all(|m| m.role != "inner"));
    server.shutdown().await;
}

struct SlowModel;

#[async_trait]
impl ConversationModel for SlowModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        tokio::time::sleep(Duration::from_secs(2)).await;
        EchoModel.generate(request).await
    }
}

#[tokio::test]
async fn concurrent_prompt_returns_lane_busy() {
    let dir = TempDir::new().unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    let server = core
        .clone()
        .serve_at(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(SlowModel) as Arc<dyn ConversationModel>,
        )
        .await
        .unwrap();
    let token = std::fs::read_to_string(core.data_dir().join("api.token")).unwrap();
    let client = ApiClient::new(format!("http://{}", server.addr), token.trim(), "cli");
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    let req = MessageRequest {
        text: "one".into(),
        mode: MessageMode::Prompt,
    };
    let req_two = MessageRequest {
        text: "two".into(),
        mode: MessageMode::Prompt,
    };
    let first = client.send_message(&session.id, &req, None);
    let second = client.send_message(&session.id, &req_two, None);
    let (a, b) = tokio::join!(first, second);
    let errs = [a.err(), b.err()];
    assert!(
        errs.iter()
            .any(|err| err.as_ref().is_some_and(|e| e.error_class() == "lane_busy")),
        "one prompt must be lane_busy, got {errs:?}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn idempotency_key_dedupes_prompt() {
    let (_dir, client, _core, server) = boot_server().await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    let req = MessageRequest {
        text: "same".into(),
        mode: MessageMode::Prompt,
    };
    let a = client
        .send_message(&session.id, &req, Some("key-1"))
        .await
        .unwrap();
    let b = client
        .send_message(&session.id, &req, Some("key-1"))
        .await
        .unwrap();
    assert_eq!(a.turn_id, b.turn_id);
    server.shutdown().await;
}

#[tokio::test]
async fn exclusive_mic_is_first_writer() {
    let (_dir, stage, _core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    let claimed = stage
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "stage".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.mic.as_deref(), Some("stage"));
    let err = web
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "web".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "resource_busy");
    server.shutdown().await;
}

#[tokio::test]
async fn approval_first_writer_wins() {
    let (_dir, stage, core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    core.plane().set_mode(ApprovalMode::AskAll).unwrap();
    let plane = core.plane();
    let task = tokio::spawn(async move {
        plane
            .authorize(&AuthzRequest {
                tool: "fs.write".into(),
                side_effects: vec!["fs.write".into()],
                sensitivity: Sensitivity::High,
                target: "notes.md".into(),
                in_workspace: false,
            })
            .await
    });
    let mut pending = None;
    for _ in 0..50 {
        let page = stage.list_approvals().await.unwrap();
        if let Some(item) = page.items.into_iter().next() {
            pending = Some(item);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pending = pending.expect("popup should be listed");
    stage.respond_approval(&pending.id, "allow").await.unwrap();
    let err = web.respond_approval(&pending.id, "deny").await.unwrap_err();
    assert_eq!(err.error_class(), "already_resolved");
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome, ene_plane::Decision::Allow);
    server.shutdown().await;
}

#[tokio::test]
async fn backup_copies_stores() {
    let (_dir, client, core, server) = boot_server().await;
    let backup = client.backup().await.unwrap();
    assert!(
        std::path::Path::new(&backup.path)
            .join("sessions.db")
            .exists()
    );
    assert!(core.data_dir().join("backups").join(&backup.id).exists());
    server.shutdown().await;
}
