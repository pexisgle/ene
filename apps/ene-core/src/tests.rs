use crate::{BootOptions, CoreDaemon, CoreError};
use ene_fiber::ProfileRow;
use ene_kernel::{ConversationModel, DisplayDepth, EchoModel, EventKind, EventPayload};
use ene_session::{
    Block, ClientId, NewEvent, NewSession, SessionCreatedBy, SessionKind, SessionStore, SoulId,
    Transaction, TurnId, TurnOrigin, TurnOutcome, TurnTrigger, abandoned_inbox, unclaimed_inbox,
    v1,
};
use std::sync::Arc;
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
        sandbox_required: false,
    })
    .unwrap();
    let web = sup
        .activate(&ProfileRow {
            row_id: "r-web".to_owned(),
            plugin: "tool.web".to_owned(),
            requires: Vec::new(),
            capabilities: Vec::new(),
            sandbox_required: false,
        })
        .unwrap();
    sup.disable_row("r-util").await;
    assert!(sup.fiber("r-web").is_some());
    assert_eq!(sup.fiber("r-web").unwrap().uid, web);
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
        .on_user_turn(soul.id, "hello", &[])
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
