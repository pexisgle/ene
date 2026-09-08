use crate::{BootOptions, CoreDaemon, CoreError};
use async_trait::async_trait;
use ene_kernel::{
    ConversationModel, DisplayDepth, EchoModel, EventKind, EventPayload, KernelError, ModelRequest,
    ToolCallingModel,
};
use ene_plugin_host::{ProfileRow, discover_plugin_script};
use ene_session::{
    Block, ClientId, NewEvent, NewSession, Role, SessionCreatedBy, SessionKind, SessionStore,
    SoulId, Transaction, TurnId, TurnOrigin, TurnOutcome, TurnTrigger, abandoned_inbox,
    unclaimed_inbox, v1,
};
use ene_tool_registry::Layer;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn boot_recovers_interrupted_turn_without_resume() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let soul = SoulId::new();
    let turn = TurnId::new();
    let session;
    {
        let store = SessionStore::open(&path, "NORMAL").await.unwrap();
        session = store
            .create_session(NewSession {
                soul_id: soul,
                body_id: None,
                kind: SessionKind::Conversation,
                delegation_id: None,
                created_by: SessionCreatedBy::Client,
            })
            .await
            .unwrap();
        store
            .commit(Transaction {
                entries: vec![
                    NewEvent::new(
                        session,
                        EventKind::TurnStart,
                        EventPayload::TurnStart {
                            v: v1(),
                            turn_id: turn,
                            lane: "dialogue".to_owned(),
                            origin: TurnOrigin::User,
                            delegation_id: None,
                            trigger: TurnTrigger::Text,
                        },
                    ),
                    NewEvent::new(
                        session,
                        EventKind::UserMessage,
                        EventPayload::UserMessage {
                            v: v1(),
                            turn_id: Some(turn),
                            blocks: vec![Block::text("half said")],
                            input_modality: "text".to_owned(),
                            client_id: ClientId::new(),
                        },
                    ),
                    NewEvent::new(
                        session,
                        EventKind::InboxEnqueued,
                        EventPayload::InboxEnqueued {
                            v: v1(),
                            lane: "dialogue".to_owned(),
                            class: ene_session::InboxClass::Wake,
                            source: ene_session::InboxSource::User,
                            ref_seq: Some(2),
                        },
                    ),
                ],
                usage: Vec::new(),
            })
            .await
            .unwrap();
        drop(store);
    }

    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.recovery().len(), 1);
    assert!(core.interruption_note().unwrap().contains("interrupted"));
    let events = core.store().load_events(session, 0).unwrap();
    assert!(unclaimed_inbox(&events).is_empty());
    assert!(!abandoned_inbox(&events).is_empty());
    assert!(events.iter().any(|e| matches!(
        e.payload,
        EventPayload::TurnEnd {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));

    let lane = core.open_lane(
        soul,
        session,
        Arc::new(EchoModel) as Arc<dyn ConversationModel>,
    );
    lane.prompt("continue?").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let blob = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(blob.contains("interrupted"));
    let events = core.store().load_events(session, 0).unwrap();
    let abandoned = abandoned_inbox(&events);
    assert!(!events.iter().any(|e| matches!(
        e.payload,
        EventPayload::InboxClaimed { entry_seq, .. } if abandoned.contains(&entry_seq)
    )));
}

#[tokio::test]
async fn exclusive_lock_rejects_second_boot() {
    let dir = TempDir::new().unwrap();
    let _first = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let Err(err) = CoreDaemon::boot(BootOptions::new(dir.path())).await else {
        panic!("expected already running")
    };
    assert!(matches!(err, CoreError::AlreadyRunning(_)));
}

#[tokio::test]
async fn job_workspace_stays_under_boot_data_dir() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let soul = core.occupants()[0].0;
    let job = core
        .host()
        .start(ene_work::StartDelegation {
            soul_id: soul,
            goal: "use the configured workspace".to_owned(),
            mode: ene_work::DelegationMode::Public,
            title: Some("workspace".to_owned()),
            brief: None,
            plan: None,
            created_from_turn: None,
            depth: 0,
            parent_id: None,
            success_criteria: Vec::new(),
            allowed_tools: Vec::new(),
        })
        .unwrap();

    let expected_root = ene_work::workspace_root(dir.path())
        .join("jobs")
        .join(soul.to_string());
    assert!(std::path::Path::new(&job.workspace_dir).starts_with(expected_root));
}

#[tokio::test]
async fn disabling_one_fiber_does_not_restart_the_core() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let sup = core.supervisor();
    sup.activate(&ProfileRow {
        row_id: "r-util".to_owned(),
        plugin: "tool.utility".to_owned(),
        requires: Vec::new(),
        capabilities: Vec::new(),
        seams: Vec::new(),
        sandbox_required: false,
        config: serde_json::Value::Null,
    })
    .unwrap();
    let web = sup
        .activate(&ProfileRow {
            row_id: "r-web".to_owned(),
            plugin: "tool.web".to_owned(),
            requires: Vec::new(),
            capabilities: Vec::new(),
            seams: Vec::new(),
            sandbox_required: false,
            config: serde_json::Value::Null,
        })
        .unwrap();
    sup.disable_row("r-util").await;
    assert!(sup.fiber("r-web").is_some());
    assert_eq!(sup.fiber("r-web").unwrap().uid, web);
    assert!(sup.registry().get("web.fetch").is_some());
    assert!(sup.surface_has_tool("web.fetch"));
    assert!(!sup.surface_has_tool("utility.hash"));
    assert_eq!(core.recovery().len(), 0);
    let soul = SoulId::new();
    let session = core
        .store()
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let lane = core.open_lane(
        soul,
        session,
        Arc::new(EchoModel) as Arc<dyn ConversationModel>,
    );
    lane.prompt("still here").await.unwrap();
    lane.wait_for_idle().await.unwrap();
}

#[tokio::test]
async fn boot_opens_companions_db() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let soul = core
        .companions()
        .create_soul(&ene_companion::NewSoul::text_only("char.boot@1"))
        .unwrap();
    core.companion()
        .on_user_turn(soul.id, "hello", &[], &[], None)
        .await
        .unwrap();
    assert!(dir.path().join("companions.db").exists());
}

#[tokio::test]
async fn boot_stage_maps_emotion_without_a_rendered_body() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let soul = core
        .companions()
        .create_soul(&ene_companion::NewSoul::text_only("char.boot@1"))
        .unwrap();
    core.present_companion(soul.id, None, ene_body::BodyCatalog::text_default())
        .unwrap();
    core.apply_body_emotion(
        soul.id,
        &ene_body::EmotionCue {
            label: "happy".into(),
            intensity: 0.7,
        },
    )
    .unwrap();
    let cmds = core.stage().bus().drain(soul.id).unwrap();
    assert!(matches!(
        cmds[0],
        ene_body::PerformanceCommand::Expression { ref label, .. } if label == "happy"
    ));
}

#[tokio::test]
async fn boot_installs_approval_plane_and_vault() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    core.vault().put("k", b"secret").unwrap();
    assert_eq!(core.vault().export("k").unwrap(), b"secret");
    core.plane().audit().verify_chain().unwrap();
}

#[tokio::test]
async fn dummy_plugin_and_harness_tools_share_registry() {
    if !std::process::Command::new("python3")
        .arg("-c")
        .arg("import sys")
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let sup = core.supervisor();
    assert!(sup.registry().get("memory.recall").is_some());
    let path = discover_plugin_script("plugin.py").expect("dummy plugin script must exist");
    sup.activate_process(
        &ProfileRow {
            row_id: "r-dummy".to_owned(),
            plugin: "tool.dummy".to_owned(),
            requires: Vec::new(),
            capabilities: Vec::new(),
            seams: Vec::new(),
            sandbox_required: false,
            config: serde_json::Value::Null,
        },
        &path,
    )
    .await
    .unwrap();
    core.plane()
        .set_mode(ene_access_control::ApprovalMode::Auto)
        .unwrap();
    let registry = sup.registry();
    assert!(registry.get("dummy.ping").is_some());
    let pong = registry
        .execute("dummy.ping", json!({"message": "shared"}), Layer::Surface)
        .await
        .unwrap();
    assert_eq!(pong.get("pong").and_then(|v| v.as_str()), Some("shared"));
    sup.unload("r-dummy").await;
    assert!(registry.get("memory.recall").is_some());
    assert!(registry.get("dummy.ping").is_none());
}

#[tokio::test]
async fn boot_reports_interrupted_job_without_resume() {
    let dir = TempDir::new().unwrap();
    let soul = SoulId::new();
    {
        let store = std::sync::Arc::new(
            ene_work::WorkStore::open(dir.path().join("companions.db")).unwrap(),
        );
        let host =
            ene_work::DelegationHost::new(std::sync::Arc::clone(&store), dir.path().to_path_buf());
        let job = host
            .start(ene_work::StartDelegation {
                soul_id: soul,
                goal: "research".into(),
                mode: ene_work::DelegationMode::Public,
                title: Some("research".into()),
                brief: None,
                plan: None,
                created_from_turn: None,
                depth: 0,
                parent_id: None,
                success_criteria: Vec::new(),
                allowed_tools: Vec::new(),
            })
            .unwrap();
        store
            .set_status(job.id, ene_work::JobStatus::Running, None)
            .unwrap();
    }
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.job_reports().len(), 1);
    assert!(core.interruption_note().unwrap().contains("research"));
    let jobs = core.work().list_jobs(soul).unwrap();
    assert_eq!(jobs[0].status, ene_work::JobStatus::Interrupted);
}

#[tokio::test]
async fn plugin_config_survives_daemon_restart() {
    let dir = TempDir::new().unwrap();
    {
        let core = CoreDaemon::boot(BootOptions::new(dir.path()))
            .await
            .unwrap();
        crate::plugin_profile::persist_applied_plugin_config(
            core.data_dir(),
            core.vault(),
            "tool.fs",
            &json!({"mode": "strict", "api_key": "sk-live"}),
            &["api_key".to_owned()],
        )
        .unwrap();
        let raw = std::fs::read_to_string(dir.path().join("plugin-config.json")).unwrap();
        assert!(raw.contains("strict"));
        assert!(!raw.contains("sk-live"));
    }
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    core.apply_plugin_profile().await;
    let row = core
        .supervisor()
        .profile_row("tool.fs")
        .expect("tool.fs row after profile apply");
    assert_eq!(row.config["mode"], "strict");
    assert_eq!(row.config["api_key"], "sk-live");
}

#[tokio::test]
async fn observation_does_not_persist_png_in_session_memory_or_audit() {
    let dir = TempDir::new().unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    let png = ene_work::MINIMAL_PNG;
    {
        let mut pipe = core.observation().lock();
        let action = pipe.evaluate(png).unwrap();
        assert!(matches!(action, ene_work::ObserveAction::Changed { .. }));
        pipe.commit_summary("tiny frame".to_owned());
        let reuse = pipe.evaluate(png).unwrap();
        assert!(matches!(reuse, ene_work::ObserveAction::Skip { .. }));
        assert!(!ene_work::contains_raw_screenshot(
            format!("{pipe:?}").as_bytes()
        ));
    }
    {
        let mut memory = core.world_state().lock();
        let snap = ene_work::observe_screen(
            &mut memory,
            &core.mind().proactive.world_state,
            "tiny frame",
            4,
        );
        let snap_json = serde_json::to_vec(&snap).unwrap();
        assert!(!ene_work::contains_raw_screenshot(&snap_json));
        assert!(!format!("{snap:?}").contains("tiny frame"));
    }
    let sessions = core.store().list_sessions(None).unwrap();
    for meta in sessions {
        let events = core.store().load_events(meta.id, 0).unwrap();
        let blob = serde_json::to_vec(&events).unwrap();
        assert!(!ene_work::contains_raw_screenshot(&blob));
    }
    let audit = core.plane().audit().records().unwrap();
    let audit_blob = serde_json::to_vec(&audit).unwrap();
    assert!(!ene_work::contains_raw_screenshot(&audit_blob));
    for name in ["sessions.db", "companions.db", "audit.db"] {
        let path = dir.path().join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            assert!(
                !ene_work::contains_raw_screenshot(&bytes),
                "{name} stored a PNG"
            );
        }
    }
}

#[tokio::test]
async fn boot_fails_on_malformed_settings_json() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("settings.json"), "{not-json").unwrap();
    let Err(err) = CoreDaemon::boot(BootOptions::new(dir.path())).await else {
        panic!("malformed settings.json must fail boot");
    };
    assert!(matches!(err, CoreError::Config(_)), "got {err:?}");
}

#[tokio::test]
async fn store_idle_timeout_from_settings_json_ends_sessions() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"store":{"sessions":{"idle_timeout_secs":1}}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.store_settings().sessions.idle_timeout_secs, 1);
    let soul = core.occupants()[0].0;
    let session = core
        .store()
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle = core.list_idle_sessions().unwrap();
    assert!(
        idle.contains(&session),
        "idle timeout from settings.json must list the session"
    );
    for id in idle {
        core.end_session(id, ene_session::SessionEndReason::IdleTimeout)
            .await
            .unwrap();
    }
    let meta = core.store().get_session(session).unwrap();
    assert!(meta.ended_at.is_some());
}

#[tokio::test]
async fn approval_policy_file_from_settings_json_is_loaded() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("team-policy.json"),
        r#"{"rules":[{"tool":"fs.write","decision":"deny"}]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"approval":{"policy_file":"team-policy.json","mode":"auto"}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.approval_settings().policy_file, "team-policy.json");
    assert_eq!(core.plane().mode(), ene_access_control::ApprovalMode::Auto);
    let policy = core.plane().policy();
    assert_eq!(policy.rules.len(), 1);
    assert_eq!(policy.rules[0].tool, "fs.write");
}

struct CountingInnerModel {
    inner_counts: Mutex<Vec<usize>>,
}

#[async_trait]
impl ConversationModel for CountingInnerModel {
    async fn generate(
        &self,
        request: ModelRequest,
    ) -> Result<ene_kernel::ModelGeneration, KernelError> {
        let count = request
            .messages
            .iter()
            .filter(|message| message.role == Role::Inner)
            .count();
        self.inner_counts.lock().push(count);
        EchoModel.generate(request).await
    }
}

#[tokio::test]
async fn mind_self_reference_window_projects_into_the_lane() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"mind":{"inner":{"self_reference_window":2}}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.mind().inner.self_reference_window, 2);
    let model = Arc::new(CountingInnerModel {
        inner_counts: Mutex::new(Vec::new()),
    });
    let soul = core.occupants()[0].0;
    let session = core
        .store()
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let lane = core.open_lane(
        soul,
        session,
        Arc::clone(&model) as Arc<dyn ConversationModel>,
    );
    for i in 0..5 {
        lane.prompt(format!("turn {i}")).await.unwrap();
        lane.wait_for_idle().await.unwrap();
    }
    let counts = model.inner_counts.lock().clone();
    let last = *counts.last().expect("model saw turns");
    assert!(
        last <= 2,
        "lane must project the configured inner window, got {counts:?}"
    );
}

#[tokio::test]
async fn harness_max_steps_one_auto_delegates_the_second_step() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"harness":{"loop":{"max_steps_per_turn":1}}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    core.plane()
        .set_mode(ene_access_control::ApprovalMode::Auto)
        .unwrap();
    assert_eq!(core.harness().loop_cfg.max_steps_per_turn, 1);
    let soul = core.occupants()[0].0;
    let session = core
        .store()
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let lane = core.open_lane(
        soul,
        session,
        Arc::new(ToolCallingModel) as Arc<dyn ConversationModel>,
    );
    lane.prompt("please calc 1+2*3").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let blob = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        blob.contains("I'll look into that."),
        "max_steps_per_turn=1 must auto-delegate: {blob}"
    );
}

#[tokio::test]
async fn boot_applies_store_sessions_synchronous_pragma() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"store":{"sessions":{"synchronous":"FULL"}}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions::new(dir.path()))
        .await
        .unwrap();
    assert_eq!(core.store().reader_synchronous().unwrap(), "FULL");
}

#[tokio::test]
async fn boot_options_synchronous_overrides_settings_json() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("settings.json"),
        r#"{"store":{"sessions":{"synchronous":"FULL"}}}"#,
    )
    .unwrap();
    let core = CoreDaemon::boot(BootOptions {
        data_dir: dir.path().to_path_buf(),
        synchronous: Some("NORMAL".to_owned()),
    })
    .await
    .unwrap();
    assert_eq!(core.store().reader_synchronous().unwrap(), "NORMAL");
}
