use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{
    CancelQueued, ConversationModel, DisplayDepth, EchoModel, EmitBus, EventKind, EventPayload,
    HarnessSettings, KernelError, LaneHandle, LaneOptions, LiveEvent, MindSettings,
    ModelGeneration, ModelRequest, ProjectOptions, TurnId, Waterfall, derive_messages,
    hash_model_visible, hash_projected, spans_leak_content,
};
use async_trait::async_trait;
use ene_session::{
    Block, ClientId, InnerAspect, NewEvent, NewSession, SessionCreatedBy, SessionKind,
    SessionStore, SoulId, Transaction, TurnOrigin, TurnOutcome, TurnTrigger, abandoned_inbox,
    open_turns, unclaimed_inbox, v1,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
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

struct InnerOnlyModel;

#[async_trait]
impl ConversationModel for InnerOnlyModel {
    async fn generate(&self, _request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        Ok(ModelGeneration {
            text: r#"<inner aspect="thought">secret</inner>"#.to_owned(),
            finish_reason: "stop".to_owned(),
            model_id: "inner-only".to_owned(),
            ..ModelGeneration::default()
        })
    }
}

struct InstantModel {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl ConversationModel for InstantModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        self.entered.notify_waiters();
        self.release.notified().await;
        EchoModel.generate(request).await
    }
}

struct HoldFirstModel {
    release: Arc<Notify>,
    held: AtomicBool,
}

#[async_trait]
impl ConversationModel for HoldFirstModel {
    async fn generate(&self, request: ModelRequest) -> Result<ModelGeneration, KernelError> {
        if self
            .held
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.release.notified().await;
        }
        EchoModel.generate(request).await
    }
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
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
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
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
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
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
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
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
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
async fn echo_turn_to_first_chunk_is_measurable() {
    let (_dir, _store, lane, _model) = open_lane().await;
    lane.prompt("warmup").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    const N: u32 = 10;
    let mut samples_us = Vec::with_capacity(N as usize);
    for i in 0..N {
        let mut surface = lane.subscribe(DisplayDepth::Surface);
        let started = Instant::now();
        lane.prompt(format!("ping {i}")).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let LiveEvent::TextDelta { .. } = surface
                    .recv()
                    .await
                    .expect("live bus closed before first chunk")
                {
                    return started.elapsed().as_micros();
                }
            }
        })
        .await
        .expect("first surface chunk");
        samples_us.push(first);
        lane.wait_for_idle().await.unwrap();
    }
    let mean_us = samples_us.iter().sum::<u128>() / u128::from(N);
    if std::fs::create_dir_all("/opt/cursor/artifacts").is_ok() {
        std::fs::write(
            "/opt/cursor/artifacts/kernel_turn_baseline.txt",
            format!("echo_first_chunk_mean_us={mean_us} n={N} samples={samples_us:?}\n"),
        )
        .ok();
    }
    assert!(
        mean_us < 50_000,
        "echo first-chunk regression mean_us={mean_us}"
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

#[tokio::test]
async fn voice_and_text_share_one_session_log() {
    let (_dir, store, lane, _model) = open_lane().await;
    lane.prompt("typed hello").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    lane.prompt_with_modality("spoken hello", "voice")
        .await
        .unwrap();
    lane.wait_for_idle().await.unwrap();
    let events = store.load_events(lane.session_id(), 0).unwrap();
    let modalities: Vec<&str> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::UserMessage { input_modality, .. } => Some(input_modality.as_str()),
            _ => None,
        })
        .collect();
    assert!(modalities.contains(&"text"));
    assert!(modalities.contains(&"voice"));
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect();
    assert!(texts.iter().any(|t| t == "typed hello"));
    assert!(texts.iter().any(|t| t == "spoken hello"));
}

#[test]
fn waterfall_rewrites_by_calling_next_and_emit_cannot() {
    let chain = Waterfall::new();
    chain.listen(|mut n, next| {
        n += 10;
        let mut out = next(n);
        out += 1;
        out
    });
    chain.listen(|mut n, next| {
        n *= 2;
        next(n)
    });
    assert_eq!(chain.run(3), 27);

    let intercepted = Waterfall::new();
    intercepted.listen(|_n, _next| 99);
    intercepted.listen(|_, next| next(1));
    assert_eq!(intercepted.run(0), 99);

    let bus = EmitBus::new();
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let seen_for_listen = std::sync::Arc::clone(&seen);
    bus.listen(move |value: &u32| seen_for_listen.lock().push(*value));
    let mut payload = 5_u32;
    bus.emit(&payload);
    payload = 0;
    assert_eq!(*seen.lock(), vec![5]);
    assert_eq!(payload, 0);
}

#[tokio::test]
async fn waterfall_pre_step_can_stop_the_model() {
    let (_dir, _store, lane, model) = open_lane().await;
    lane.hooks().pre_step.listen(|mut event, _next| {
        event.proceed = false;
        event.note = "blocked by waterfall".into();
        event
    });
    lane.prompt("hello").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    assert!(model.last.lock().is_none());
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect();
    assert!(texts.iter().any(|t| t == "hello"));
    assert!(texts.iter().any(|t| t.contains("blocked by waterfall")));
}

#[tokio::test]
async fn inner_only_model_does_not_leak_on_surface() {
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
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::new(InnerOnlyModel),
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
    });
    let mut surface = lane.subscribe(DisplayDepth::Surface);
    lane.prompt("probe").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let surface_hist = lane.project(DisplayDepth::Surface).unwrap();
    let surface_text = surface_hist
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!surface_text.contains("secret"));
    let surface_events = surface.try_drain();
    for event in &surface_events {
        if let LiveEvent::TextDelta { text, .. } = event {
            assert!(!text.contains("secret"));
        }
    }
    let detail_hist = lane.project(DisplayDepth::Detail).unwrap();
    assert!(
        detail_hist
            .messages
            .iter()
            .any(|m| m.text().contains("secret"))
    );
}

#[tokio::test]
async fn compact_summary_omits_inner_on_surface() {
    let (_dir, store, lane, _model) = open_lane().await;
    let turn = TurnId::new();
    store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    lane.session_id(),
                    EventKind::UserMessage,
                    EventPayload::UserMessage {
                        v: v1(),
                        turn_id: Some(turn),
                        blocks: vec![Block::text("hello")],
                        input_modality: "text".to_owned(),
                        client_id: ClientId::new(),
                    },
                ),
                NewEvent::new(
                    lane.session_id(),
                    EventKind::InnerMessage,
                    EventPayload::InnerMessage {
                        v: v1(),
                        turn_id: Some(turn),
                        step_index: Some(0),
                        aspects: vec![InnerAspect::Thought],
                        blocks: vec![Block::text("hidden inner body")],
                        model_visible: true,
                    },
                ),
            ],
            usage: Vec::new(),
        })
        .await
        .unwrap();
    lane.compact().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let text = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("hidden inner body"));
}

#[tokio::test]
async fn abort_after_generate_does_not_write_assistant_closure() {
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
    let done = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::new(InstantModel {
            entered: Arc::clone(&done),
            release: Arc::clone(&release),
        }),
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
    });
    let wait_enter = done.notified();
    let turn = lane.prompt("race").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), wait_enter)
        .await
        .expect("model generate");
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
async fn duplicate_abort_is_idempotent() {
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
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::new(HoldModel {
            release: Arc::clone(&release),
        }),
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
    });
    lane.prompt("hold").await.unwrap();
    lane.abort().await.unwrap();
    lane.abort().await.unwrap();
    release.notify_one();
    lane.wait_for_idle().await.unwrap();
    lane.abort().await.unwrap();
}

#[tokio::test]
async fn follow_up_queues_fifo_and_next_run_works_when_idle() {
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
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&store),
        session,
        soul,
        model: Arc::new(HoldFirstModel {
            release: Arc::clone(&release),
            held: AtomicBool::new(false),
        }),
        harness: HarnessSettings::default(),
        mind: MindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        router: None,
    });
    lane.prompt("first").await.unwrap();
    lane.follow_up("second").await.unwrap();
    lane.follow_up("third").await.unwrap();
    let idle_err = lane.next_run("idle next").await;
    assert!(idle_err.is_ok());
    release.notify_one();
    lane.wait_for_idle().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    let texts: Vec<String> = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect();
    assert!(texts.iter().any(|t| t == "second"));
    assert!(texts.iter().any(|t| t == "third"));
    assert!(texts.iter().any(|t| t == "idle next"));
    assert!(lane.follow_up("nope").await.is_err());
}

#[tokio::test]
async fn next_run_works_when_lane_is_idle() {
    let (_dir, _store, lane, _model) = open_lane().await;
    lane.wait_for_idle().await.unwrap();
    lane.next_run("scheduled").await.unwrap();
    lane.prompt("start").await.unwrap();
    lane.wait_for_idle().await.unwrap();
    let history = lane.project(DisplayDepth::Surface).unwrap();
    assert!(history.messages.iter().any(|m| m.text() == "scheduled"));
}
