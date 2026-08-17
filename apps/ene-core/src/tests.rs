use crate::{BootOptions, CoreDaemon, CoreError};
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
