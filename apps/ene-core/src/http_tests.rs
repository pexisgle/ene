use crate::{BootOptions, CoreDaemon};
use async_trait::async_trait;
use base64::Engine;
use ene_api::{
    ApiClient, ClaimResourceRequest, CreateSessionRequest, EndSessionRequest, HistoryResponse,
    MessageMode, MessageRequest, ResourceKind, RestoreRequest,
};
use ene_companion::{
    CompanionStore, MemoryKind, MemoryScope, MemorySource, NewMemory, content_digest,
    install_archive, pack_archive,
};
use ene_kernel::{
    ConversationModel, EchoModel, KernelError, ModelGeneration, ModelRequest, Span,
    spans_leak_content,
};
use ene_plane::{ApprovalMode, AuthzRequest, Sensitivity};
use ene_session::{EventPayload, SessionId, TurnOutcome};
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

async fn first_soul_id(client: &ApiClient) -> String {
    client
        .list_souls()
        .await
        .unwrap()
        .items
        .first()
        .expect("boot seeds souls")
        .id
        .clone()
}

struct ParkingJobModel;

#[async_trait]
impl ConversationModel for ParkingJobModel {
    async fn generate(&self, _: ModelRequest) -> Result<ModelGeneration, KernelError> {
        std::future::pending().await
    }
}

async fn boot_server() -> (TempDir, ApiClient, Arc<CoreDaemon>, crate::ServerHandle) {
    let dir = TempDir::new().unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    core.set_job_model(Arc::new(ParkingJobModel));
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

async fn wait_history_contains(client: &ApiClient, session: &str, needle: &str) -> HistoryResponse {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let history = client.history(session, "surface").await.unwrap();
        if history
            .messages
            .iter()
            .any(|message| message.text.contains(needle))
        {
            return history;
        }
        assert!(
            Instant::now() < deadline,
            "history did not contain {needle:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn start_job(core: &CoreDaemon, soul: ene_session::SoulId, goal: &str) -> ene_work::Job {
    core.host()
        .start(ene_work::StartDelegation {
            soul_id: soul,
            goal: goal.into(),
            mode: ene_work::DelegationMode::Public,
            title: Some(goal.into()),
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 0,
            parent_id: None,
        })
        .unwrap()
}

fn record_metric(name: &str, body: impl AsRef<[u8]>) {
    let dir = std::path::Path::new("/opt/cursor/artifacts");
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    std::fs::write(dir.join(name), body).ok();
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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
async fn exclusive_mic_second_claims_after_release() {
    let (_dir, stage, _core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    stage
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "stage".into(),
            },
        )
        .await
        .unwrap();
    let busy = web
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "web".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(busy.error_class(), "resource_busy");
    let released = stage.release_resource(ResourceKind::Mic).await.unwrap();
    assert!(released.mic.is_none());
    let claimed = web
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "web".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(claimed.mic.as_deref(), Some("web"));
    let still_busy = stage
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "stage".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(still_busy.error_class(), "resource_busy");
    server.shutdown().await;
}

#[tokio::test]
async fn barge_in_aborts_busy_lane() {
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: None,
        })
        .await
        .unwrap();
    let started = client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "slow turn".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    let turn = started.turn_id.expect("prompt must return turn_id");
    client.barge_in(&session.id).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let sid = SessionId::from_str(&session.id).unwrap();
    loop {
        let events = core.store().load_events(sid, 0).unwrap();
        if events.iter().any(|event| {
            matches!(
                &event.payload,
                EventPayload::TurnEnd {
                    outcome: TurnOutcome::Interrupted,
                    turn_id,
                    ..
                } if turn_id.to_string() == turn
            )
        }) {
            break;
        }
        assert!(Instant::now() < deadline, "turn was not interrupted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let history = client.history(&session.id, "surface").await.unwrap();
    assert!(
        !history
            .messages
            .iter()
            .any(|message| message.role == "assistant" && message.text.contains("ack: slow turn")),
        "aborted turn must not write assistant closure"
    );
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
    assert_eq!(err.error_class(), "forbidden");
    let cli = ApiClient::new(stage.base(), stage.token(), "cli");
    let err = cli.respond_approval(&pending.id, "deny").await.unwrap_err();
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
    assert!(
        std::path::Path::new(&backup.path)
            .join("vault.key")
            .exists()
    );
    assert!(core.data_dir().join("backups").join(&backup.id).exists());
    server.shutdown().await;
}

#[tokio::test]
async fn backup_restore_roundtrip_and_unknown_id() {
    let (_dir, client, core, server) = boot_server().await;
    let missing = client
        .restore(&RestoreRequest {
            id: "20990101T120000".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(missing.error_class(), "not_found");
    let backup = client.backup().await.unwrap();
    client
        .restore(&RestoreRequest { id: backup.id })
        .await
        .unwrap();
    assert!(core.data_dir().join("backups").join("pre-restore").is_dir());
    assert_eq!(client.health().await.unwrap().status, "ok");
    server.shutdown().await;
}

#[tokio::test]
async fn restore_rejects_active_jobs() {
    let (_dir, client, core, server) = boot_server().await;
    let soul = core.occupants()[0].0;
    let job = core
        .host()
        .start(ene_work::StartDelegation {
            soul_id: soul,
            goal: "running".into(),
            mode: ene_work::DelegationMode::Public,
            title: Some("running".into()),
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 0,
            parent_id: None,
        })
        .unwrap();
    core.work()
        .set_status(job.id, ene_work::JobStatus::Running, None)
        .unwrap();
    let backup = client.backup().await.unwrap();
    let err = client
        .restore(&RestoreRequest { id: backup.id })
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "job_busy");
    server.shutdown().await;
}

#[tokio::test]
async fn job_runner_completes_queued_work_on_its_own_lane() {
    let (_dir, _client, core, server) = boot_server().await;
    core.set_job_model(Arc::new(EchoModel));
    let soul = core.occupants()[0].0;
    let job = start_job(&core, soul, "research a quiet cafe");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let current = core.work().get_job(job.id).unwrap().unwrap();
        if current.status == ene_work::JobStatus::Completed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "job stayed {:?}, expected completed",
            current.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mail = core.work().mailbox(job.id).unwrap();
    assert!(
        mail.iter()
            .any(|(direction, kind, _)| direction == "child_to_parent" && kind == "complete"),
        "runner must complete via the job lane, got {mail:?}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn boot_loads_settings_json_token_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"core":{"server":{"token_file":"custom.token","bind":"127.0.0.1:0"}}}"#,
    )
    .unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    assert_eq!(core.settings().server.token_file, "custom.token");
    assert_eq!(core.workspace_dir(), dir.path().join("workspace"));
    assert!(core.workspace_dir().is_dir());
    let server = core
        .clone()
        .serve_with(Arc::new(EchoModel) as Arc<dyn ConversationModel>)
        .await
        .unwrap();
    assert!(dir.path().join("custom.token").is_file());
    let ready: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("api.json")).unwrap())
            .unwrap();
    assert_eq!(ready["token_file"], "custom.token");
    server.shutdown().await;
}

#[tokio::test]
async fn http_forget_memory_is_audited() {
    let (_dir, client, core, server) = boot_server().await;
    let souls = client.list_souls().await.unwrap();
    let soul = ene_session::SoulId::from_str(&souls.items[0].id).unwrap();
    let memory = core
        .companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "picnic".into(),
            content: "we planned a picnic".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    let listed = client
        .list_memories(&souls.items[0].id, None)
        .await
        .unwrap();
    assert!(
        listed
            .items
            .iter()
            .any(|item| item.id == memory.id.to_string())
    );
    client.delete_memory(&memory.id.to_string()).await.unwrap();
    let after = client
        .list_memories(&souls.items[0].id, None)
        .await
        .unwrap();
    assert!(
        after
            .items
            .iter()
            .all(|item| item.id != memory.id.to_string())
    );
    let audit = client.audit().await.unwrap();
    let blob = audit.to_string();
    assert!(
        blob.contains("forget") || blob.contains(&memory.id.to_string()),
        "forget must be audited: {blob}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn turn_logs_context_sources_from_registry() {
    let (dir, client, core, server) = boot_server().await;
    let souls = client.list_souls().await.unwrap();
    let soul = ene_session::SoulId::from_str(&souls.items[0].id).unwrap();
    core.companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "picnic".into(),
            content: "we planned a picnic".into(),
            confidence: 0.9,
            salience: 0.8,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    core.companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Shared,
            kind: MemoryKind::UserProfile,
            title: "name".into(),
            content: "the user's name is Ada".into(),
            confidence: 0.9,
            salience: 0.9,
            source: MemorySource::UserStated,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    core.companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Private,
            kind: MemoryKind::Commitment,
            title: "call".into(),
            content: "call Ada on Friday".into(),
            confidence: 0.8,
            salience: 0.7,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    let skill_dir = dir.path().join("skills/research");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: research\ndescription: Investigate a topic\n---\nLook things up.\n",
    )
    .unwrap();
    let mcp_dir = core.workspace_dir().join("mcp-context");
    std::fs::create_dir_all(&mcp_dir).unwrap();
    std::fs::write(mcp_dir.join("notes.md"), "MCP note: picnic weather.\n").unwrap();
    core.work()
        .insert_job(&ene_work::NewJob {
            id: None,
            soul_id: soul,
            title: "bookmark picnic".into(),
            goal: "summarize picnic research".into(),
            mode: ene_work::DelegationMode::Public,
            workspace_dir: core.workspace_dir().display().to_string(),
            created_from_turn: None,
            plan: None,
            brief: None,
        })
        .unwrap();

    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: souls.items[0].id.clone(),
            title: Some("context assembly".into()),
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "remind me about the picnic".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;

    let sid = ene_session::SessionId::from_str(&session.id).unwrap();
    let mut texts = std::collections::BTreeMap::<String, String>::new();
    let mut keys = Vec::new();
    for event in core.store().load_events(sid, 0).unwrap() {
        if let EventPayload::ContextSystemMessage {
            source_key, blocks, ..
        } = event.payload
        {
            let text = blocks
                .iter()
                .filter_map(ene_session::Block::as_text)
                .collect::<Vec<_>>()
                .join("");
            keys.push(source_key.clone());
            texts.insert(source_key, text);
        }
    }
    let platform = keys
        .iter()
        .position(|k| k == "platform_contract")
        .expect("platform_contract");
    let identity = keys
        .iter()
        .position(|k| k == "identity_kernel")
        .expect("identity_kernel");
    let affect = keys
        .iter()
        .position(|k| k == "character_state")
        .expect("character_state");
    let semantic = keys
        .iter()
        .position(|k| k == "memory.semantic")
        .expect("memory.semantic");
    let profile = keys
        .iter()
        .position(|k| k == "memory.user_profile")
        .expect("memory.user_profile");
    let commits = keys
        .iter()
        .position(|k| k == "memory.commitments")
        .expect("memory.commitments");
    let skills = keys
        .iter()
        .position(|k| k == "skills.catalog")
        .expect("skills.catalog");
    let mcp = keys
        .iter()
        .position(|k| k == "mcp.resources")
        .expect("mcp.resources");
    let jobs = keys
        .iter()
        .position(|k| k == "delegation.active")
        .expect("delegation.active");
    assert!(platform < identity && identity < affect);
    assert!(affect < semantic && semantic < profile && profile < commits);
    assert!(commits < skills && skills < mcp && mcp < jobs);
    assert!(texts["memory.semantic"].contains("picnic"));
    assert!(texts["memory.user_profile"].contains("Ada"));
    assert!(texts["memory.commitments"].contains("Friday"));
    assert!(texts["skills.catalog"].contains("research"));
    assert!(texts["mcp.resources"].contains("picnic weather"));
    assert!(texts["delegation.active"].contains("bookmark picnic"));
    assert!(texts["character_state"].contains("mood="));
    server.shutdown().await;
}

#[tokio::test]
async fn serve_binds_seamed_approve_model() {
    let (_dir, _client, core, server) = boot_server().await;
    assert!(
        core.plane().has_approve_model(),
        "production plane must expose ai.tasks.approve"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn hybrid_recall_falls_back_without_embedding_and_ranks_with_vector() {
    let (_dir, client, core, server) = boot_server().await;
    let souls = client.list_souls().await.unwrap();
    let soul = ene_session::SoulId::from_str(&souls.items[0].id).unwrap();
    let near = core
        .companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "apple pie".into(),
            content: "baked dessert".into(),
            confidence: 0.9,
            salience: 0.5,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    core.companions()
        .insert_memory(NewMemory {
            soul_id: soul,
            scope: MemoryScope::Private,
            kind: MemoryKind::Episodic,
            title: "zebra stripes".into(),
            content: "black and white".into(),
            confidence: 0.9,
            salience: 0.5,
            source: MemorySource::Extraction,
            source_seq: None,
            expires_at: None,
        })
        .unwrap();
    core.companions()
        .set_embedding(near.id, &[1.0, 0.0])
        .unwrap();
    let lexical = core.companion().recall(soul, "xyzzy").unwrap();
    assert!(
        lexical.is_empty(),
        "unconfigured embedding query must stay lexical: {lexical:?}"
    );
    let hybrid = core
        .companion()
        .recall_ranked(soul, "xyzzy", Some(&[1.0, 0.0]))
        .unwrap();
    assert_eq!(hybrid[0].title, "apple pie");
    server.shutdown().await;
}

#[tokio::test]
async fn plugin_supervisor_waterfall_stops_a_turn() {
    let (_dir, client, core, server) = boot_server().await;
    let row = core
        .supervisor()
        .active_row_ids()
        .into_iter()
        .next()
        .expect("plugin profile loads at least one fiber");
    core.supervisor()
        .listen_pre_step(&row, |mut event, _next| {
            event.proceed = false;
            event.note = "blocked by fiber".into();
            event
        })
        .unwrap();
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "hello".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_history_contains(&client, &session.id, "blocked by fiber").await;
    server.shutdown().await;
}

#[tokio::test]
async fn export_default_omits_inner() {
    let (_dir, client, _core, server) = boot_server().await;
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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

#[tokio::test]
async fn web_client_cannot_mutate_memory_or_settings() {
    let (_dir, stage, _core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    let souls = stage.list_souls().await.unwrap();
    let soul = &souls.items[0].id;
    let memories = stage.list_memories(soul, None).await.unwrap();
    if let Some(memory) = memories.items.first() {
        let err = web
            .patch_memory(&memory.id, &ene_api::MemoryPatch::default())
            .await
            .unwrap_err();
        assert_eq!(err.error_class(), "forbidden");
        let err = web.delete_memory(&memory.id).await.unwrap_err();
        assert_eq!(err.error_class(), "forbidden");
    }
    let err = web
        .patch_settings(&serde_json::json!({ "theme": "light" }))
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "forbidden");
    let err = web.backup().await.unwrap_err();
    assert_eq!(err.error_class(), "forbidden");
    let err = web
        .restore(&RestoreRequest {
            id: "20240101T000000".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "forbidden");
    server.shutdown().await;
}

#[test]
fn web_ui_is_read_only_in_source() {
    let html = include_str!("../web/index.html");
    assert!(html.contains("/api/v1/souls"));
    assert!(html.contains("display_name"));
    assert!(
        !html.contains("method: \"PATCH\"") && !html.contains("method: \"DELETE\""),
        "Web UI must not PATCH/DELETE in page scripts"
    );
    assert!(
        !html.contains("JSON.stringify(memories")
            && !html.contains("JSON.stringify(jobs")
            && !html.contains("JSON.stringify(affect")
            && !html.contains("log(detail, JSON.stringify"),
        "detail pane must render fields, not JSON dumps"
    );
    assert!(html.contains("History (detail)"));
    assert!(html.contains("Memories"));
    assert!(html.contains("PAD"));
}

#[tokio::test]
async fn http_spans_and_schema_and_anon_health() {
    let (_dir, client, core, server) = boot_server().await;
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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
    assert!(
        schema.get("properties").is_some()
            || schema.get("$defs").is_some()
            || schema.get("$schema").is_some()
            || schema.get("title").is_some(),
        "generated schema must describe settings: {schema}"
    );
    client
        .patch_settings(&serde_json::json!({ "theme": "dark" }))
        .await
        .unwrap();
    assert!(core.data_dir().join("settings.json").is_file());
    let settings = client.settings().await.unwrap();
    assert_eq!(settings["overlay"]["theme"], "dark");
    assert!(settings.get("effective").is_some());
    let audit = client.audit().await.unwrap();
    assert!(audit.get("items").is_some());
    let anon = ApiClient::new(client.base(), "", "web");
    assert_eq!(anon.health().await.unwrap().status, "ok");
    let err = anon.list_souls().await.unwrap_err();
    assert_eq!(err.error_class(), "unauthorized");
    server.shutdown().await;
}

#[tokio::test]
async fn usage_ledger_records_completed_turn() {
    let (_dir, client, _core, server) = boot_server().await;
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "ledger ping".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session.id).await;
    let usage = client.usage(Some(&session.id)).await.unwrap();
    assert!(usage.input_tokens > 0 || usage.output_tokens > 0);
    assert!(usage.rows >= 1);
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
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
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

#[tokio::test]
async fn two_souls_keep_isolated_sessions_and_stage_occupants() {
    let (_dir, client, _core, server) = boot_server().await;
    let souls = client.list_souls().await.unwrap();
    assert!(
        souls.items.len() >= 2,
        "boot must present two companions, got {}",
        souls.items.len()
    );
    let soul_a = souls.items[0].id.clone();
    let soul_b = souls.items[1].id.clone();
    assert_ne!(soul_a, soul_b);

    let session_a = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_a.clone(),
            title: Some("alpha lane".into()),
        })
        .await
        .unwrap();
    let session_b = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_b.clone(),
            title: Some("beta lane".into()),
        })
        .await
        .unwrap();
    assert_ne!(session_a.id, session_b.id);
    assert_eq!(session_a.soul_id, soul_a);
    assert_eq!(session_b.soul_id, soul_b);

    client
        .send_message(
            &session_a.id,
            &MessageRequest {
                text: "token-mango-unique".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session_a.id).await;
    client
        .send_message(
            &session_b.id,
            &MessageRequest {
                text: "token-kiwi-unique".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    wait_assistant(&client, &session_b.id).await;

    let hist_a = client.history(&session_a.id, "surface").await.unwrap();
    let hist_b = client.history(&session_b.id, "surface").await.unwrap();
    let texts_a: Vec<&str> = hist_a.messages.iter().map(|m| m.text.as_str()).collect();
    let texts_b: Vec<&str> = hist_b.messages.iter().map(|m| m.text.as_str()).collect();
    assert!(
        texts_a
            .iter()
            .any(|text| text.contains("token-mango-unique")),
        "soul A history: {texts_a:?}"
    );
    assert!(
        !texts_a
            .iter()
            .any(|text| text.contains("token-kiwi-unique")),
        "soul A leaked B: {texts_a:?}"
    );
    assert!(
        texts_b
            .iter()
            .any(|text| text.contains("token-kiwi-unique")),
        "soul B history: {texts_b:?}"
    );
    assert!(
        !texts_b
            .iter()
            .any(|text| text.contains("token-mango-unique")),
        "soul B leaked A: {texts_b:?}"
    );

    let stage = client.stage().await.unwrap();
    assert!(
        stage.occupants.iter().any(|item| item.soul_id == soul_a),
        "stage occupants: {:?}",
        stage.occupants
    );
    assert!(
        stage.occupants.iter().any(|item| item.soul_id == soul_b),
        "stage occupants: {:?}",
        stage.occupants
    );
    server.shutdown().await;
}

#[tokio::test]
async fn import_shipped_alicia_vrm_exposes_parseable_avatar() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/characters/Alicia/AliciaSolid.vrm");
    assert!(
        path.is_file(),
        "shipped AliciaSolid.vrm is required for VRM acceptance"
    );
    let vrm = std::fs::read(&path).unwrap();
    assert!(vrm.starts_with(b"glTF"));
    assert!(
        vrm.windows(8).any(|window| window == b"VRMC_vrm"),
        "shipped Alicia VRM must declare VRMC_vrm"
    );

    let (_dir, client, _core, server) = boot_server().await;
    let mut files = sample_char_files();
    files.insert(
        "body/body.toml".into(),
        b"[body]\nkind = \"vrm\"\navatar = \"avatar/model.vrm\"\n".to_vec(),
    );
    files.insert("body/avatar/model.vrm".into(), vrm);
    let zip = pack_archive(&stamp_digest(files)).unwrap();
    let imported = client
        .import_character_archive_b64(&base64::engine::general_purpose::STANDARD.encode(&zip))
        .await
        .unwrap();
    let soul_id = imported.soul_id.expect("soul");
    let soul = client.get_soul(&soul_id).await.unwrap();
    let avatar = soul.avatar_path.expect("avatar_path");
    let installed = std::fs::read(&avatar).unwrap();
    assert!(installed.starts_with(b"glTF"));
    assert!(installed.windows(8).any(|window| window == b"VRMC_vrm"));
    let stage = client.stage().await.unwrap();
    assert!(
        stage
            .occupants
            .iter()
            .any(|occupant| occupant.soul_id == soul_id && occupant.avatar_path.is_some()),
        "stage occupants: {:?}",
        stage.occupants
    );
    server.shutdown().await;
}

fn sample_char_files() -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    files.insert(
        "manifest.toml".into(),
        b"[package]\nkind = \"character\"\nid = \"char.mychar\"\nversion = \"1.0.0\"\nformat_version = 1\ndisplay_name = \"My Character\"\n\n[contents]\nsoul = \"embedded\"\nbody = \"embedded\"\n\n[integrity]\ndigest = \"\"\n".to_vec(),
    );
    files.insert(
        "soul/soul.toml".into(),
        b"[identity]\nname = \"Ene\"\n\n[affect]\nbaseline = { valence = 0.2, arousal = 0.1, dominance = 0.0, trust = 0.3, affinity = 0.3, irritation = 0.0, curiosity = 0.4, fatigue = 0.0 }\n".to_vec(),
    );
    files.insert("soul/persona.md".into(), b"You are Ene.".to_vec());
    files.insert(
        "body/body.toml".into(),
        b"[body]\nkind = \"text\"\n\n[expressions]\navailable = [\"happy\", \"calm\"]\n".to_vec(),
    );
    files
}

fn stamp_digest(mut files: BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Vec<u8>> {
    let digest = content_digest(&files);
    let manifest = String::from_utf8(files.get("manifest.toml").unwrap().clone()).unwrap();
    files.insert(
        "manifest.toml".into(),
        manifest
            .replace("digest = \"\"", &format!("digest = \"{digest}\""))
            .into_bytes(),
    );
    files
}

#[tokio::test]
async fn import_rejects_path_outside_import_dirs() {
    let (_dir, client, _core, server) = boot_server().await;
    let err = client.import_character("/etc/passwd").await.unwrap_err();
    assert!(
        err.error_class() == "invalid_message" || err.error_class() == "fault",
        "unexpected class: {}",
        err.error_class()
    );
    server.shutdown().await;
}

#[tokio::test]
async fn restore_keeps_backed_up_session_readable() {
    let (_dir, client, _core, server) = boot_server().await;
    let soul_id = first_soul_id(&client).await;
    let first = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_id.clone(),
            title: Some("restore-me".into()),
        })
        .await
        .unwrap();
    let backup = client.backup().await.unwrap();
    let _second = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: Some("after-backup".into()),
        })
        .await
        .unwrap();
    client
        .restore(&RestoreRequest { id: backup.id })
        .await
        .unwrap();
    let restored = client.get_session(&first.id).await.unwrap();
    assert_eq!(restored.title.as_deref(), Some("restore-me"));
    server.shutdown().await;
}

#[tokio::test]
async fn web_cannot_release_stage_mic_via_body_spoof() {
    let (_dir, stage, _core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    stage
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "ignored".into(),
            },
        )
        .await
        .unwrap();
    let released = web.release_resource(ResourceKind::Mic).await.unwrap();
    assert_eq!(released.mic.as_deref(), Some("stage"));
    server.shutdown().await;
}

#[tokio::test]
async fn create_session_rejects_unknown_soul() {
    let (_dir, client, _core, server) = boot_server().await;
    let err = client
        .create_session(&CreateSessionRequest {
            soul_id: ene_session::SoulId::new().to_string(),
            title: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "not_found");
    server.shutdown().await;
}

#[tokio::test]
async fn http_character_import_list_export_roundtrip() {
    let (_dir, client, _core, server) = boot_server().await;
    let zip = pack_archive(&stamp_digest(sample_char_files())).unwrap();
    let imported = client
        .import_character_archive_b64(&base64::engine::general_purpose::STANDARD.encode(&zip))
        .await
        .unwrap();
    assert_eq!(imported.id, "char.mychar");
    assert_eq!(imported.version, "1.0.0");
    assert_eq!(imported.kind, "character");
    assert!(
        imported.soul_id.is_some(),
        "import of a character package must activate a soul"
    );

    let listed = client.list_characters().await.unwrap();
    assert!(
        listed
            .items
            .iter()
            .any(|item| item.id == "char.mychar" && item.version == "1.0.0"),
        "imported character must appear in list: {:?}",
        listed.items
    );

    let exported = client.export_character("char.mychar").await.unwrap();
    assert_eq!(
        exported.get("id").and_then(|v| v.as_str()),
        Some("char.mychar")
    );
    let archive_b64 = exported
        .get("archive_b64")
        .and_then(|v| v.as_str())
        .expect("export must return archive_b64");
    let zip = base64::engine::general_purpose::STANDARD
        .decode(archive_b64)
        .expect("archive_b64 must decode");
    let roundtrip_dir = TempDir::new().unwrap();
    let store = CompanionStore::open(roundtrip_dir.path().join("companions.db")).unwrap();
    let installed = install_archive(
        &store,
        &roundtrip_dir.path().join("characters"),
        &zip,
        32 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(installed.id, "char.mychar");
    assert_eq!(installed.version, "1.0.0");
    server.shutdown().await;
}

#[tokio::test]
async fn import_vrm_package_exposes_avatar_path_on_soul_and_stage() {
    let (_dir, client, _core, server) = boot_server().await;
    let mut files = sample_char_files();
    files.insert(
        "body/body.toml".into(),
        b"[body]\nkind = \"vrm\"\navatar = \"avatar/model.vrm\"\n".to_vec(),
    );
    files.insert("body/avatar/model.vrm".into(), b"not-a-real-vrm".to_vec());
    let zip = pack_archive(&stamp_digest(files)).unwrap();
    let imported = client
        .import_character_archive_b64(&base64::engine::general_purpose::STANDARD.encode(&zip))
        .await
        .unwrap();
    let soul_id = imported.soul_id.expect("soul");
    let soul = client.get_soul(&soul_id).await.unwrap();
    assert_eq!(soul.package_id.as_deref(), Some("char.mychar@1.0.0"));
    let avatar = soul.avatar_path.expect("avatar_path");
    assert!(
        avatar.ends_with("avatar/model.vrm"),
        "unexpected avatar_path: {avatar}"
    );
    let stage = client.stage().await.unwrap();
    assert!(
        stage
            .occupants
            .iter()
            .any(|occupant| occupant.soul_id == soul_id && occupant.avatar_path.is_some()),
        "stage occupants: {:?}",
        stage.occupants
    );
    server.shutdown().await;
}

#[tokio::test]
async fn activate_character_is_idempotent() {
    let (_dir, client, _core, server) = boot_server().await;
    let zip = pack_archive(&stamp_digest(sample_char_files())).unwrap();
    let imported = client
        .import_character_archive_b64(&base64::engine::general_purpose::STANDARD.encode(&zip))
        .await
        .unwrap();
    let first = imported.soul_id.expect("soul from import");
    let again = client.activate_character(&imported.id).await.unwrap();
    assert_eq!(again.soul_id.as_deref(), Some(first.as_str()));
    server.shutdown().await;
}

#[tokio::test]
async fn web_client_cannot_patch_settings() {
    let (_dir, stage, _core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    let err = web
        .patch_settings(&serde_json::json!({ "theme": "dark" }))
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "forbidden");
    server.shutdown().await;
}

#[tokio::test]
async fn web_cannot_resolve_approval() {
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
    let err = web
        .respond_approval(&pending.id, "allow")
        .await
        .unwrap_err();
    assert_eq!(err.error_class(), "forbidden");
    stage.respond_approval(&pending.id, "allow").await.unwrap();
    let outcome = task.await.unwrap().unwrap();
    assert_eq!(outcome, ene_plane::Decision::Allow);
    server.shutdown().await;
}

#[tokio::test]
async fn job_report_lands_only_in_owning_soul_session() {
    let (_dir, client, core, server) = boot_server().await;
    let occupants = core.occupants();
    assert!(occupants.len() >= 2, "boot seeds two occupants");
    let soul_a = occupants[0].0;
    let soul_b = occupants[1].0;
    let session_a = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_a.to_string(),
            title: None,
        })
        .await
        .unwrap();
    let session_b = client
        .create_session(&CreateSessionRequest {
            soul_id: soul_b.to_string(),
            title: None,
        })
        .await
        .unwrap();
    let job = start_job(&core, soul_a, "alpha notes");
    core.host().complete(job.id, "alpha done").unwrap();
    wait_history_contains(&client, &session_a.id, "alpha done").await;
    let other = client.history(&session_b.id, "surface").await.unwrap();
    assert!(
        !other
            .messages
            .iter()
            .any(|message| message.text.contains("alpha done")),
        "job speech must not fan out to another soul's session"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn web_release_does_not_drain_stage_speech_gate() {
    let (_dir, stage, core, server) = boot_server().await;
    let web = ApiClient::new(stage.base(), stage.token(), "web");
    let soul = core.occupants()[0].0;
    let session = stage
        .create_session(&CreateSessionRequest {
            soul_id: soul.to_string(),
            title: None,
        })
        .await
        .unwrap();
    stage
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "ignored".into(),
            },
        )
        .await
        .unwrap();
    let job = start_job(&core, soul, "queued speech");
    let queued = core.host().complete(job.id, "held until gap").unwrap();
    assert_eq!(queued.inner_intent.as_deref(), Some("complete_queued"));
    let released = web.release_resource(ResourceKind::Mic).await.unwrap();
    assert_eq!(released.mic.as_deref(), Some("stage"));
    assert!(core.host().speech_gate().user_speaking());
    tokio::time::sleep(Duration::from_millis(150)).await;
    let history = stage.history(&session.id, "surface").await.unwrap();
    assert!(
        !history
            .messages
            .iter()
            .any(|message| message.text.contains("held until gap"))
    );
    stage.release_resource(ResourceKind::Mic).await.unwrap();
    wait_history_contains(&stage, &session.id, "held until gap").await;
    server.shutdown().await;
}

#[tokio::test]
async fn ws_disconnect_while_holding_mic_drains_speech() {
    let (_dir, client, core, server) = boot_server().await;
    let soul = core.occupants()[0].0;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id: soul.to_string(),
            title: None,
        })
        .await
        .unwrap();
    let socket = client.events("surface", Some(&session.id)).await.unwrap();
    client
        .claim_resource(
            ResourceKind::Mic,
            &ClaimResourceRequest {
                client_id: "ignored".into(),
            },
        )
        .await
        .unwrap();
    let job = start_job(&core, soul, "ws drain");
    let queued = core.host().complete(job.id, "after hangup").unwrap();
    assert_eq!(queued.inner_intent.as_deref(), Some("complete_queued"));
    drop(socket);
    wait_history_contains(&client, &session.id, "after hangup").await;
    let exclusive = client.exclusive().await.unwrap();
    assert!(exclusive.mic.is_none());
    server.shutdown().await;
}

#[tokio::test]
async fn serve_echo_chat_and_strip_api_key_from_saved_settings() {
    let dir = TempDir::new().unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    let server = core
        .clone()
        .serve_with(Arc::new(EchoModel) as Arc<dyn ConversationModel>)
        .await
        .unwrap();
    let token = std::fs::read_to_string(core.data_dir().join("api.token")).unwrap();
    let client = ApiClient::new(format!("http://{}", server.addr), token.trim(), "desktop");
    let soul_id = first_soul_id(&client).await;
    let session = client
        .create_session(&CreateSessionRequest {
            soul_id,
            title: None,
        })
        .await
        .unwrap();
    client
        .send_message(
            &session.id,
            &MessageRequest {
                text: "ping".into(),
                mode: MessageMode::Prompt,
                input_modality: None,
            },
            None,
        )
        .await
        .unwrap();
    let history = wait_assistant(&client, &session.id).await;
    assert!(
        history
            .messages
            .iter()
            .any(|message| message.text.contains("ack: ping"))
    );

    let patched = client
        .patch_settings(&serde_json::json!({
            "ai": {
                "tasks": {
                    "chat": {
                        "plugin": "provider.openai_compat",
                        "model": "gpt-test",
                        "api_key": "sk-must-not-persist"
                    }
                }
            }
        }))
        .await
        .unwrap();
    assert!(patched.pointer("/ai/tasks/chat/api_key").is_none());
    let saved = std::fs::read_to_string(core.data_dir().join("settings.json")).unwrap();
    assert!(
        !saved.contains("sk-must-not-persist"),
        "vault-bound keys must not land in settings.json"
    );
    let settings = client.settings().await.unwrap();
    assert_eq!(
        settings.pointer("/effective/ai/tasks/chat/plugin"),
        Some(&serde_json::json!("provider.openai_compat"))
    );
    assert_eq!(settings.get("ai_chat_key_set"), None);
    assert_eq!(
        settings.pointer("/effective/ai_chat_key_set"),
        Some(&serde_json::json!(true))
    );

    let tools = client.list_tools().await.unwrap();
    let names: Vec<&str> = tools.items.iter().map(|tool| tool.name.as_str()).collect();
    assert!(
        names.contains(&"utility.hash"),
        "harness tools must load with the plugin profile: {names:?}"
    );
    assert!(names.contains(&"app.screenshot"));

    let empty = client
        .listen(
            &session.id,
            &ene_api::ListenRequest {
                pcm: vec![0.0; 16],
                sample_rate: 16_000,
            },
        )
        .await
        .unwrap();
    assert!(empty.turn_id.is_none());
    server.shutdown().await;
}

#[tokio::test]
async fn plugins_profile_minimal_unloads_non_utility_harness() {
    let (_dir, client, core, server) = boot_server().await;
    let settings = client.settings().await.unwrap();
    assert_eq!(
        settings.pointer("/effective/plugins/profile"),
        Some(&serde_json::json!("desktop"))
    );
    let before = client.list_tools().await.unwrap();
    let names: Vec<&str> = before.items.iter().map(|tool| tool.name.as_str()).collect();
    assert!(names.contains(&"app.screenshot"), "{names:?}");
    assert!(names.contains(&"utility.hash"), "{names:?}");

    client
        .patch_settings(&serde_json::json!({
            "plugins": { "profile": "minimal" }
        }))
        .await
        .unwrap();
    assert_eq!(core.plugins().lock().profile, "minimal");
    let after = client.list_tools().await.unwrap();
    let names: Vec<&str> = after.items.iter().map(|tool| tool.name.as_str()).collect();
    assert!(names.contains(&"utility.hash"), "{names:?}");
    assert!(
        !names.contains(&"app.screenshot"),
        "minimal profile must drop app: {names:?}"
    );
    assert!(
        !names.iter().any(|name| name.starts_with("fs.")),
        "minimal profile must drop fs: {names:?}"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn mcp_document_round_trips_through_http() {
    let (_dir, client, core, server) = boot_server().await;
    let empty = client.mcp().await.unwrap();
    assert!(empty.servers.is_empty());
    let saved = client
        .put_mcp(&ene_api::McpDocument {
            servers: vec![ene_api::McpServerView {
                id: "fixture".into(),
                transport: "stdio".into(),
                command: Some("__ene_missing_mcp__".into()),
                args: vec!["-c".into(), "pass".into()],
                url: None,
                enabled: true,
            }],
        })
        .await
        .unwrap();
    assert_eq!(saved.servers.len(), 1);
    assert_eq!(saved.servers[0].id, "fixture");
    let disk = std::fs::read_to_string(core.data_dir().join("mcp.json")).unwrap();
    assert!(disk.contains("fixture"));
    let listed = core.work().list_mcp().unwrap();
    assert_eq!(listed[0].command.as_deref(), Some("__ene_missing_mcp__"));
    server.shutdown().await;
}

#[tokio::test]
async fn list_provider_models_rejects_unknown_plugin_and_task() {
    let (_dir, client, _core, server) = boot_server().await;
    let unknown_plugin = client
        .list_provider_models(&ene_api::ListProviderModelsRequest {
            plugin: "provider.not_in_catalog".into(),
            task: "chat".into(),
            ..ene_api::ListProviderModelsRequest::default()
        })
        .await
        .expect_err("plugin");
    assert_eq!(unknown_plugin.error_class(), "invalid_message");
    let unknown_task = client
        .list_provider_models(&ene_api::ListProviderModelsRequest {
            plugin: "provider.openai_compat".into(),
            task: "not-a-task".into(),
            ..ene_api::ListProviderModelsRequest::default()
        })
        .await
        .expect_err("task");
    assert_eq!(unknown_task.error_class(), "invalid_message");
    server.shutdown().await;
}

#[tokio::test]
async fn get_settings_effective_ai_ignores_stub_disk_overlay() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"ai":{"tasks":{"chat":{"plugin":"provider.openai_compat","model":"live-model"}}}}"#,
    )
    .unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    let server = core
        .clone()
        .serve_with(Arc::new(EchoModel) as Arc<dyn ConversationModel>)
        .await
        .unwrap();
    let token = std::fs::read_to_string(core.data_dir().join("api.token")).unwrap();
    let client = ApiClient::new(format!("http://{}", server.addr), token.trim(), "desktop");
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"mind":{"language":"ja"},"ai":{"tasks":{"chat":{"plugin":"","model":""}}}}"#,
    )
    .unwrap();
    let settings = client.settings().await.unwrap();
    assert_eq!(
        settings.pointer("/effective/ai/tasks/chat/plugin"),
        Some(&serde_json::json!("provider.openai_compat"))
    );
    assert_eq!(
        settings.pointer("/effective/ai/tasks/chat/model"),
        Some(&serde_json::json!("live-model"))
    );
    assert_eq!(
        settings.pointer("/overlay/ai/tasks/chat/plugin"),
        Some(&serde_json::json!(""))
    );
    server.shutdown().await;
}

#[tokio::test]
async fn patch_settings_writes_data_dir_settings_json() {
    let dir = TempDir::new().unwrap();
    let core = Arc::new(
        CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap(),
    );
    let server = core
        .clone()
        .serve_with(Arc::new(EchoModel) as Arc<dyn ConversationModel>)
        .await
        .unwrap();
    let token = std::fs::read_to_string(core.data_dir().join("api.token")).unwrap();
    let client = ApiClient::new(format!("http://{}", server.addr), token.trim(), "desktop");
    client
        .patch_settings(&serde_json::json!({
            "ai": { "tasks": { "chat": { "plugin": "provider.gguf", "model": "local" } } }
        }))
        .await
        .unwrap();
    let path = core.data_dir().join("settings.json");
    assert!(path.is_file());
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        saved.pointer("/ai/tasks/chat/plugin"),
        Some(&serde_json::json!("provider.gguf"))
    );
    server.shutdown().await;
}
