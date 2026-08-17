use std::sync::Arc;

use crate::{
    CancelQueued, ConversationModel, DisplayDepth, EchoModel, EventKind, EventPayload,
    HarnessSettings, KernelError, LaneHandle, LaneOptions, LiveEvent, MindSettings,
    ModelGeneration, ModelRequest, ProjectOptions, TurnId, derive_messages, hash_model_visible,
    hash_projected, spans_leak_content,
};
use async_trait::async_trait;
use ene_session::{
    Block, ClientId, NewEvent, NewSession, SessionCreatedBy, SessionKind, SessionStore, SoulId,
    Transaction, TurnOrigin, TurnOutcome, TurnTrigger, abandoned_inbox, open_turns,
    unclaimed_inbox, v1,
};
use parking_lot::Mutex;
use tempfile::TempDir;
use tokio::sync::Notify;

struct RecordingModel {
    inner: EchoModel,
    last: Mutex<Option<ModelRequest>>,
}

#[async_trait]
impl ConversationModel for RecordingModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        *self.last.lock() = Some(request.clone());
        self.inner.generate(request).await
    }
}

struct HoldModel {
    release: Arc<Notify>,
}

#[async_trait]
impl ConversationModel for HoldModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        self.release.notified().await;
        EchoModel.generate(request).await
    }
}

async fn open_lane() -> (TempDir, Arc<SessionStore>, LaneHandle, Arc<RecordingModel>) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let soul = SoulId::new();
    let session = store
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let model = Arc::new(RecordingModel {
        inner: EchoModel,
        last: Mutex::new(None),
    });
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::clone(&model) as Arc<dyn ConversationModel>,
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
    });
    (dir, store, lane, model)
}

#[tokio::test]
async fn text_turn_is_logged_and_projected() {
    let (_dir, store, lane, _model) = open_lane().await;
    let mut surface = lane.subscribe(DisplayDepth::Surface);
    let turn = lane.prompt("hello").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect();
    assert!(texts.iter().any(|t| t == "hello"));
    assert!(texts.iter().any(|t| t.contains("ack: hello")));
    let events = store.load_events(lane.session_id(), 0).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::TurnStart))
    );
    assert!(events.iter().any(|e| matches!(
        e.payload,
        EventPayload::TurnEnd {
            outcome: TurnOutcome::Completed,
            turn_id,
            ..
        } if turn_id == turn
    )));
    let live = surface.try_drain();
    assert!(
        live.iter()
            .any(|event| matches!(event, LiveEvent::TextDelta { .. }))
    );
}

#[tokio::test]
async fn model_visible_hash_matches_logged_projection() {
    let (_dir, store, lane, model) = open_lane().await;
    lane.prompt("ping").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let captured = model.last.lock().clone().expect("model saw a request");
    let events = store.load_events(lane.session_id(), 0).unwrap();
    let until_generate: Vec<_> = events
        .into_iter()
        .take_while(|event| {
            !matches!(
                event.kind,
                EventKind::AssistantMessage
                    | EventKind::AssistantThinking
                    | EventKind::InnerMessage
            )
        })
        .collect();
    let replayed = derive_messages(&until_generate, ProjectOptions::model_visible(8));
    assert_eq!(
        hash_projected(&replayed).unwrap(),
        hash_model_visible(&captured.messages).unwrap()
    );
    assert_eq!(replayed.messages, captured.messages);
}

#[tokio::test]
async fn surface_live_subscription_does_not_receive_inner() {
    let (_dir, _store, lane, _model) = open_lane().await;
    let mut surface = lane.subscribe(DisplayDepth::Surface);
    let mut detail = lane.subscribe(DisplayDepth::Detail);
    lane.prompt("hi").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let surface_events = surface.try_drain();
    let detail_events = detail.try_drain();
    assert!(surface_events.iter().all(|event| !matches!(
        event,
        LiveEvent::InnerMessage { .. } | LiveEvent::ThinkingDelta { .. }
    )));
    assert!(
        detail_events
            .iter()
            .any(|event| matches!(event, LiveEvent::InnerMessage { .. }))
    );
    let surface_hist = lane.project(DisplayDepth::Surface).unwrap();
    assert!(
        !surface_hist
            .messages
            .iter()
            .any(|m| m.role == ene_session::Role::Inner)
    );
    let detail_hist = lane.project(DisplayDepth::Detail).unwrap();
    assert!(
        detail_hist
            .messages
            .iter()
            .any(|m| m.role == ene_session::Role::Inner)
    );
}

#[tokio::test]
async fn prompt_while_busy_returns_lane_busy() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let soul = SoulId::new();
    let session = store
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let release = Arc::new(Notify::new());
    let model = Arc::new(HoldModel {
        release: Arc::clone(&release),
    });
    let lane = LaneHandle::spawn(LaneOptions {
        store,
        session,
        soul,
        model,
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
    });
    let first = lane.prompt("one").await.unwrap();
    let busy = lane.prompt("two").await.unwrap_err();
    assert!(matches!(busy, KernelError::LaneBusy { turn_id } if turn_id == first));
    release.notify_one();
    lane.wait_for_idle().await.unwrap();
}

#[tokio::test]
async fn abort_does_not_write_assistant_closure() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(
        SessionStore::open(dir.path().join("sessions.db"), "NORMAL")
            .await
            .unwrap(),
    );
    let soul = SoulId::new();
    let session = store
        .create_session(NewSession {
            soul_id: soul,
            body_id: None,
            kind: SessionKind::Conversation,
            delegation_id: None,
            created_by: SessionCreatedBy::Client,
        })
        .await
        .unwrap();
    let release = Arc::new(Notify::new());
    let model = Arc::new(HoldModel {
        release: Arc::clone(&release),
    });
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model,
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
    });
    let turn = lane.prompt("hold").await.unwrap();
    lane.abort().await.unwrap();
    release.notify_one();
    lane.wait_for_idle().await.unwrap();
    let events = store.load_events(session, 0).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, EventKind::AssistantMessage))
    );
    assert!(events.iter().any(|e| matches!(
        e.payload,
        EventPayload::TurnEnd {
            outcome: TurnOutcome::Interrupted,
            turn_id,
            ..
        } if turn_id == turn
    )));
}

#[tokio::test]
async fn crash_recovery_is_reported_and_not_resumed() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("sessions.db");
    let soul;
    let session;
    let turn = TurnId::new();
    {
        let store = SessionStore::open(&path, "NORMAL").await.unwrap();
        soul = SoulId::new();
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
    let store = Arc::new(SessionStore::open(&path, "NORMAL").await.unwrap());
    let reports = store.recover_interrupted().await.unwrap();
    assert_eq!(reports.len(), 1);
    let events = store.load_events(session, 0).unwrap();
    assert!(open_turns(&events).is_empty());
    assert!(unclaimed_inbox(&events).is_empty());
    assert!(!abandoned_inbox(&events).is_empty());
    let model = Arc::new(RecordingModel {
        inner: EchoModel,
        last: Mutex::new(None),
    });
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::clone(&model) as Arc<dyn ConversationModel>,
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: reports,
    });
    lane.prompt("what happened").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let captured = model.last.lock().clone().unwrap();
    let blob = captured
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(blob.contains("interrupted"));
    let events = store.load_events(session, 0).unwrap();
    assert!(!events.iter().any(|e| matches!(
        e.payload,
        EventPayload::InboxClaimed { entry_seq, .. } if abandoned_inbox(&events).contains(&entry_seq)
    )));
}

#[tokio::test]
async fn cancel_queued_not_found_is_success() {
    let (_dir, _store, lane, _model) = open_lane().await;
    assert_eq!(
        lane.cancel_queued(99).await.unwrap(),
        CancelQueued::NotFound
    );
}

#[tokio::test]
async fn observe_spans_do_not_leak_content() {
    let (_dir, _store, lane, _model) = open_lane().await;
    lane.prompt("secret prompt text").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    assert!(!spans_leak_content(&lane.observe().snapshot()));
}

#[tokio::test]
async fn compact_keeps_original_rows() {
    let (_dir, store, lane, _model) = open_lane().await;
    lane.prompt("old").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let summary_seq = lane.compact().await.unwrap();
    assert!(summary_seq > 0);
    let events = store.load_events(lane.session_id(), 0).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e.kind, EventKind::UserMessage))
    );
    let history = lane.project(DisplayDepth::Surface).unwrap();
    assert!(
        history.messages.iter().any(|m| m.text().contains("old")
            || m.text().contains("summary")
            || !m.text().is_empty())
    );
}
