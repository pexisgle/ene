use crate::{BootOptions, CoreDaemon};
use async_trait::async_trait;
use ene_api::{
    ApiClient, ClaimResourceRequest, CreateSessionRequest, EndSessionRequest, HistoryResponse,
    MessageMode, MessageRequest, ResourceKind,
};
use ene_kernel::{
    ConversationModel, EchoModel, KernelError, ModelGeneration, ModelRequest, Span,
    spans_leak_content,
};
use ene_plane::{ApprovalMode, AuthzRequest, Sensitivity};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
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

async fn wait_assistant(client: &ApiClient, session: &str) -> HistoryResponse {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let history = client.history(session, "surface").await.unwrap();
        if history
            .messages
            .iter()
            .any(|message| message.role == "assistant")
        {
            return history;
        }
        assert!(
            Instant::now() < deadline,
            "assistant message did not land in surface history"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn record_metric(name: &str, body: impl AsRef<[u8]>) {
    std::fs::create_dir_all("/opt/cursor/artifacts").unwrap();
    std::fs::write(format!("/opt/cursor/artifacts/{name}"), body).unwrap();
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
                input_modality: None,
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
        input_modality: None,
    };
    let req_two = MessageRequest {
        text: "two".into(),
        mode: MessageMode::Prompt,
        input_modality: None,
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
        input_modality: None,
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

#[tokio::test]
async fn export_default_omits_inner() {
    let (_dir, client, _core, server) = boot_server().await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "hello inner".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    let exported = client.export_session(&session.id).await.unwrap();
    let blob = exported.to_string();
    assert!(
        !blob.contains("inner/message"),
        "default export must omit inner events: {blob}"
    );
    let detail = client.history(&session.id, "detail").await.unwrap();
    assert!(
        detail
            .messages
            .iter()
            .any(|message| message.role == "inner"),
        "detail history must include inner"
    );
    let surface = client.history(&session.id, "surface").await.unwrap();
    assert!(
        surface
            .messages
            .iter()
            .all(|message| message.role != "inner")
    );
    server.shutdown().await;
}

#[tokio::test]
async fn fork_leaves_original_session_intact() {
    let (_dir, client, _core, server) = boot_server().await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: Some("origin".into()),
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "keep me".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    let forked = client.fork_session(&session.id).await.unwrap();
    assert_ne!(forked.id, session.id);
    let origin = client.get_session(&session.id).await.unwrap();
    assert_eq!(origin.id, session.id);
    let origin_history = client.history(&session.id, "surface").await.unwrap();
    assert!(
        origin_history
            .messages
            .iter()
            .any(|message| message.text.contains("keep me"))
    );
    server.shutdown().await;
}

#[test]
fn web_ui_cannot_mutate_memory_or_settings() {
    let html = include_str!("../web/index.html");
    assert!(html.contains("/api/v1/souls/"));
    assert!(html.contains("memories"));
    assert!(html.contains("/affect"));
    assert!(
        !html.contains("method: \"PATCH\"") && !html.contains("method: \"DELETE\""),
        "Web UI must not PATCH/DELETE (settings and memory mutation stay off the Web client)"
    );
    let stage_main = include_str!("../../ene-stage/src/main.rs");
    let stage_app = include_str!("../../ene-stage/src/stage_app.rs");
    assert!(stage_main.contains("eframe"));
    assert!(stage_app.contains("show_viewport_immediate"));
    assert!(!stage_main.to_ascii_lowercase().contains("webview"));
    assert!(!stage_app.to_ascii_lowercase().contains("webview"));
}

#[tokio::test]
async fn http_spans_and_schema_and_anon_health() {
    let (_dir, client, _core, server) = boot_server().await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "secret prompt text".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    let spans = client.diag_spans().await.unwrap();
    let mapped: Vec<Span> = spans
        .items
        .into_iter()
        .map(|span| Span {
            name: span.name,
            duration: span
                .duration_ms
                .map(|ms| Duration::from_millis(u64::try_from(ms).unwrap_or(u64::MAX))),
            attrs: span.attrs,
        })
        .collect();
    assert!(!spans_leak_content(&mapped));
    let schema = client.settings_schema().await.unwrap();
    assert!(schema.is_object(), "settings schema must be JSON");
    let audit = client.audit().await.unwrap();
    assert!(audit.get("items").is_some());
    let anon = ApiClient::new(client.base(), "", "web");
    assert_eq!(anon.health().await.unwrap().status, "ok");
    let err = anon.list_souls().await.unwrap_err();
    assert_eq!(err.error_class(), "unauthorized");
    server.shutdown().await;
}

#[tokio::test]
async fn minimal_http_baselines_are_measurable() {
    let started = Instant::now();
    let (_dir, client, core, server) = boot_server().await;
    let boot_ms = started.elapsed().as_millis();
    client.health().await.unwrap();
    let health_started = Instant::now();
    const N: u32 = 20;
    for _ in 0..N {
        client.health().await.unwrap();
    }
    let health_mean_us = health_started.elapsed().as_micros() / u128::from(N);
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap();
    let prompt_started = Instant::now();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "offline ping".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    let prompt_ms = prompt_started.elapsed().as_millis();
    let db_bytes =
        std::fs::metadata(core.data_dir().join("sessions.db")).map_or(0, |meta| meta.len());
    record_metric(
        "minimal_http_baseline.txt",
        format!(
            "boot_to_ready_ms={boot_ms} health_mean_us={health_mean_us} echo_prompt_ms={prompt_ms} sessions_db_bytes={db_bytes}\n"
        ),
    );
    assert!(boot_ms < 5_000, "boot_to_ready_ms={boot_ms}");
    assert!(health_mean_us < 20_000, "health_mean_us={health_mean_us}");
    assert!(prompt_ms < 2_000, "echo_prompt_ms={prompt_ms}");
    server.shutdown().await;
}

#[tokio::test]
async fn boot_seeds_two_souls_and_session_ops() {
    let (_dir, client, core, server) = boot_server().await;
    let souls = client.list_souls().await.unwrap();
    assert!(
        souls.items.len() >= 2,
        "boot must present two companions, got {}",
        souls.items.len()
    );
    let stage = client.stage().await.unwrap();
    assert!(stage.occupants.len() >= 2);
    let affect = client.soul_affect(&souls.items[0].id).await.unwrap();
    assert!(!affect.mood_label.is_empty());
    assert!(affect.valence.is_finite());
    assert!(affect.arousal.is_finite());
    assert!(affect.dominance.is_finite());

    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: souls.items[0].id.clone(),
            title: Some("picnic plans".into()),
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "unique pineapple picnic".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "spoken weather".into(),
                mode: MessageMode::Prompt,
                input_modality: Some("voice".into()),
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;

    let sid = ene_session::SessionId::from_str(&session.id).unwrap();
    let events = core.store().load_events(sid, 0).unwrap();
    let modalities: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            ene_session::EventPayload::UserMessage { input_modality, .. } => {
                Some(input_modality.as_str())
            }
            _ => None,
        })
        .collect();
    assert!(modalities.contains(&"text"));
    assert!(modalities.contains(&"voice"));

    let found = client
        .search_sessions(None, Some("pineapple"))
        .await
        .unwrap();
    assert!(
        found.items.iter().any(|item| item.id == session.id),
        "surface search must find the picnic turn"
    );

    let split = client.split_session(&session.id).await.unwrap();
    assert_eq!(split.previous.id, session.id);
    assert_eq!(split.previous.end_reason.as_deref(), Some("explicit"));
    assert_ne!(split.session.id, session.id);
    assert_eq!(split.session.soul_id, session.soul_id);
    assert!(split.session.ended_at.is_none());

    let ended = client
        .end_session(
            &split.session.id,
            &EndSessionRequest {
                reason: "idle_timeout".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(ended.end_reason.as_deref(), Some("idle_timeout"));

    let barge = client.barge_in(&split.session.id).await;
    assert!(
        barge.is_ok()
            || barge
                .as_ref()
                .err()
                .is_some_and(|err| err.error_class() == "no_active_operation")
    );
    server.shutdown().await;
}
