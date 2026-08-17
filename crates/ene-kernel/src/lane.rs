//! Dialogue lane: `prompt` / `steer` / `follow_up` / `abort` / `compact`.

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Notify, mpsc, oneshot};
use tracing::warn;

use crate::config::{HarnessSettings, MindSettings};
use crate::context::{ContextRegistry, format_recovery_note};
use crate::error::{CancelQueued, KernelError};
use crate::inner::{derive_thought_from_thinking, model_visible_for, split_surface_and_inner};
use crate::live::{LiveBus, LiveEvent, LiveSubscription};
use crate::model::{ConversationModel, ModelGeneration, ModelRequest};
use crate::observe::ObserveHandle;
use crate::router::{SurfaceRouter, SurfaceToolOutcome};
use ene_session::{
    Block, CallId, ClientId, DelegationId, DisplayDepth, EventKind, EventPayload, InboxClass,
    InboxSource, LaneId, NewEvent, NewUsage, ProjectOptions, RecoveryReport, SessionId,
    SessionStore, SoulId, StepOutcome, ToolStatus, Transaction, TurnId, TurnOrigin, TurnOutcome,
    TurnTrigger, derive_messages, hash_model_visible, hash_projected, v1,
};

enum LaneCmd {
    Prompt {
        text: String,
        reply: oneshot::Sender<Result<TurnId, KernelError>>,
    },
    Steer {
        text: String,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    FollowUp {
        text: String,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    NextRun {
        text: String,
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    Abort {
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
    Compact {
        reply: oneshot::Sender<Result<u64, KernelError>>,
    },
    CancelQueued {
        entry_id: u64,
        reply: oneshot::Sender<Result<CancelQueued, KernelError>>,
    },
    RecordUsage {
        usage: NewUsage,
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
    WaitIdle {
        reply: oneshot::Sender<Result<(), KernelError>>,
    },
}

struct QueuedWake {
    seq: u64,
    text: String,
}

struct QueuedInject {
    seq: u64,
    text: String,
}

struct LaneState {
    store: Arc<SessionStore>,
    session: SessionId,
    soul: SoulId,
    model: Arc<dyn ConversationModel>,
    observe: ObserveHandle,
    live: LiveBus,
    mind: MindSettings,
    max_steps: u32,
    context: ContextRegistry,
    router: Option<Arc<dyn SurfaceRouter>>,
    running: Option<RunningTurn>,
    queued_wake: Option<QueuedWake>,
    queued_inject: Option<QueuedInject>,
    consumed_queue: Vec<u64>,
}

struct RunningTurn {
    turn: TurnId,
    cancel: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
}

/// Options for opening a dialogue lane.
pub struct LaneOptions {
    /// Session store (owns `sessions.db`).
    pub store: Arc<SessionStore>,
    /// Session this lane drives.
    pub session: SessionId,
    /// Soul that owns the session.
    pub soul: SoulId,
    /// Conversation model.
    pub model: Arc<dyn ConversationModel>,
    /// Optional harness overrides.
    pub harness: HarnessSettings,
    /// Inner-window and related mind settings.
    pub mind: MindSettings,
    /// Reports from `recover_interrupted` to inject into the next turn.
    pub recovery: Vec<RecoveryReport>,
    /// Surface tool router. `None` keeps the lane speech-only.
    pub router: Option<Arc<dyn SurfaceRouter>>,
}

/// Handle to the single dialogue lane.
#[derive(Clone)]
pub struct LaneHandle {
    tx: mpsc::UnboundedSender<LaneCmd>,
    live: LiveBus,
    observe: ObserveHandle,
    session: SessionId,
    store: Arc<SessionStore>,
}

impl LaneHandle {
    /// Spawn the lane actor.
    #[must_use]
    pub fn spawn(opts: LaneOptions) -> Self {
        let live = LiveBus::new();
        let observe = ObserveHandle::new(256);
        let store = Arc::clone(&opts.store);
        let session = opts.session;
        let (tx, rx) = mpsc::unbounded_channel();
        let mut context = ContextRegistry::new();
        context.set_interruption_note(format_recovery_note(&opts.recovery));
        let state = LaneState {
            store: opts.store,
            session: opts.session,
            soul: opts.soul,
            model: opts.model,
            observe: observe.clone(),
            live: live.clone(),
            mind: opts.mind,
            max_steps: opts.harness.loop_cfg.max_steps_per_turn,
            context,
            router: opts.router,
            running: None,
            queued_wake: None,
            queued_inject: None,
            consumed_queue: Vec::new(),
        };
        tokio::spawn(lane_actor(state, rx));
        Self {
            tx,
            live,
            observe,
            session,
            store,
        }
    }

    /// Session this lane drives.
    #[must_use]
    pub fn session_id(&self) -> SessionId {
        self.session
    }

    /// Subscribe to live events. Depth is enforced server-side (I-38).
    #[must_use]
    pub fn subscribe(&self, depth: DisplayDepth) -> LiveSubscription {
        self.live.subscribe(depth)
    }

    /// Observe span ring.
    #[must_use]
    pub fn observe(&self) -> ObserveHandle {
        self.observe.clone()
    }

    /// Reconstruct history from the log.
    pub fn project(
        &self,
        depth: DisplayDepth,
    ) -> Result<ene_session::ProjectedHistory, KernelError> {
        let events = self.store.load_events(self.session, 0)?;
        Ok(derive_messages(
            &events,
            ProjectOptions::for_depth(depth, 8),
        ))
    }

    /// Start a user turn. Fails with [`KernelError::LaneBusy`] if a turn is running.
    pub async fn prompt(&self, text: impl Into<String>) -> Result<TurnId, KernelError> {
        self.ask(|reply| LaneCmd::Prompt {
            text: text.into(),
            reply,
        })
        .await
    }

    /// Queue a correction for the running turn (`inject`; generation is not cut).
    pub async fn steer(&self, text: impl Into<String>) -> Result<u64, KernelError> {
        self.ask(|reply| LaneCmd::Steer {
            text: text.into(),
            reply,
        })
        .await
    }

    /// Queue a follow-up wake for after the current turn.
    pub async fn follow_up(&self, text: impl Into<String>) -> Result<u64, KernelError> {
        self.ask(|reply| LaneCmd::FollowUp {
            text: text.into(),
            reply,
        })
        .await
    }

    /// Reserve input that is accepted only after the current operation ends.
    pub async fn next_run(&self, text: impl Into<String>) -> Result<u64, KernelError> {
        self.ask(|reply| LaneCmd::NextRun {
            text: text.into(),
            reply,
        })
        .await
    }

    /// Abort the running turn. No assistant closure is written (I-21).
    pub async fn abort(&self) -> Result<(), KernelError> {
        self.ask(|reply| LaneCmd::Abort { reply }).await
    }

    /// Compact the session log. Original rows remain (I-23).
    pub async fn compact(&self) -> Result<u64, KernelError> {
        self.ask(|reply| LaneCmd::Compact { reply }).await
    }

    /// Drop a queued follow-up / steer / `next_run` entry.
    pub async fn cancel_queued(&self, entry_id: u64) -> Result<CancelQueued, KernelError> {
        self.ask(|reply| LaneCmd::CancelQueued { entry_id, reply })
            .await
    }

    /// Append a usage-ledger row (dialogue-adjacent LLM calls).
    pub async fn record_usage(&self, usage: NewUsage) -> Result<(), KernelError> {
        self.ask(|reply| LaneCmd::RecordUsage { usage, reply })
            .await
    }

    /// Wait until no turn is running and no queued wake remains.
    pub async fn wait_for_idle(&self) -> Result<(), KernelError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(LaneCmd::WaitIdle { reply })
            .map_err(|_| KernelError::ShuttingDown)?;
        tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .map_err(|_| KernelError::ShuttingDown)?
            .map_err(|_| KernelError::ShuttingDown)?
    }

    async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, KernelError>>) -> LaneCmd,
    ) -> Result<T, KernelError> {
        let (reply, rx) = oneshot::channel();
        self.tx
            .send(make(reply))
            .map_err(|_| KernelError::ShuttingDown)?;
        rx.await.map_err(|_| KernelError::ShuttingDown)?
    }
}

async fn lane_actor(mut state: LaneState, mut rx: mpsc::UnboundedReceiver<LaneCmd>) {
    let mut idle_waiters: Vec<oneshot::Sender<Result<(), KernelError>>> = Vec::new();
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<TurnFinish>();
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                dispatch_cmd(&mut state, cmd, &done_tx, &mut idle_waiters).await;
            }
            finished = done_rx.recv() => {
                let Some(finished) = finished else { break };
                on_turn_finished(&mut state, finished, &done_tx, &mut idle_waiters).await;
            }
        }
    }
}

async fn dispatch_cmd(
    state: &mut LaneState,
    cmd: LaneCmd,
    done_tx: &mpsc::UnboundedSender<TurnFinish>,
    idle_waiters: &mut Vec<oneshot::Sender<Result<(), KernelError>>>,
) {
    match cmd {
        LaneCmd::Prompt { text, reply } => {
            let result = start_prompt(state, text, done_tx).await;
            drop(reply.send(result));
        }
        LaneCmd::Steer { text, reply } => {
            let result = enqueue_inject(state, text).await;
            drop(reply.send(result));
        }
        LaneCmd::FollowUp { text, reply } | LaneCmd::NextRun { text, reply } => {
            let result = enqueue_wake(state, text).await;
            drop(reply.send(result));
        }
        LaneCmd::Abort { reply } => {
            let result = request_abort(state);
            drop(reply.send(result));
        }
        LaneCmd::Compact { reply } => {
            let result = run_compact(state).await;
            drop(reply.send(result));
        }
        LaneCmd::CancelQueued { entry_id, reply } => {
            let result = cancel_queued(state, entry_id).await;
            drop(reply.send(result));
        }
        LaneCmd::RecordUsage { usage, reply } => {
            let result = state
                .store
                .commit(Transaction {
                    entries: Vec::new(),
                    usage: vec![usage],
                })
                .await
                .map(|_| ())
                .map_err(KernelError::from);
            drop(reply.send(result));
        }
        LaneCmd::WaitIdle { reply } => {
            if state.running.is_none() && state.queued_wake.is_none() {
                drop(reply.send(Ok(())));
            } else {
                idle_waiters.push(reply);
            }
        }
    }
}

async fn on_turn_finished(
    state: &mut LaneState,
    finished: TurnFinish,
    done_tx: &mpsc::UnboundedSender<TurnFinish>,
    idle_waiters: &mut Vec<oneshot::Sender<Result<(), KernelError>>>,
) {
    if state
        .running
        .as_ref()
        .is_some_and(|running| running.turn == finished.turn)
    {
        state.running = None;
    }
    if let Some(wake) = state.queued_wake.take() {
        state.consumed_queue.push(wake.seq);
        let inject = state.queued_inject.take();
        if let Some(ref inject) = inject {
            state.consumed_queue.push(inject.seq);
        }
        if let Err(err) = start_follow_turn(state, wake, inject, done_tx).await {
            warn!(error = %err, "queued wake failed to start");
        }
        return;
    }
    for waiter in idle_waiters.drain(..) {
        drop(waiter.send(Ok(())));
    }
}

async fn start_prompt(
    state: &mut LaneState,
    text: String,
    done_tx: &mpsc::UnboundedSender<TurnFinish>,
) -> Result<TurnId, KernelError> {
    if let Some(running) = &state.running {
        return Err(KernelError::lane_busy(running.turn));
    }
    let text = validate_text(&text)?;
    let turn = TurnId::new();
    let entries = vec![
        NewEvent::new(
            state.session,
            EventKind::UserMessage,
            EventPayload::UserMessage {
                v: v1(),
                turn_id: Some(turn),
                blocks: vec![Block::text(&text)],
                input_modality: "text".to_owned(),
                client_id: ClientId::new(),
            },
        ),
        NewEvent::new(
            state.session,
            EventKind::InboxEnqueued,
            EventPayload::InboxEnqueued {
                v: v1(),
                lane: LaneId::Dialogue.as_str(),
                class: InboxClass::Wake,
                source: InboxSource::User,
                ref_seq: None,
            },
        ),
    ];
    let committed = state
        .store
        .commit(Transaction {
            entries,
            usage: Vec::new(),
        })
        .await?;
    let enqueue_seq = committed.seqs.get(1).copied().unwrap_or(0);
    let inject = state.queued_inject.take();
    spawn_turn(
        state,
        TurnSpawn {
            turn,
            origin: TurnOrigin::User,
            claim_seq: Some(enqueue_seq),
            inject,
        },
        done_tx,
    )
    .await?;
    Ok(turn)
}

async fn start_follow_turn(
    state: &mut LaneState,
    wake: QueuedWake,
    inject: Option<QueuedInject>,
    done_tx: &mpsc::UnboundedSender<TurnFinish>,
) -> Result<TurnId, KernelError> {
    let turn = TurnId::new();
    state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::UserMessage,
                EventPayload::UserMessage {
                    v: v1(),
                    turn_id: Some(turn),
                    blocks: vec![Block::text(&wake.text)],
                    input_modality: "text".to_owned(),
                    client_id: ClientId::new(),
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    spawn_turn(
        state,
        TurnSpawn {
            turn,
            origin: TurnOrigin::User,
            claim_seq: Some(wake.seq),
            inject,
        },
        done_tx,
    )
    .await?;
    Ok(turn)
}

struct TurnSpawn {
    turn: TurnId,
    origin: TurnOrigin,
    claim_seq: Option<u64>,
    inject: Option<QueuedInject>,
}

async fn spawn_turn(
    state: &mut LaneState,
    spawn: TurnSpawn,
    done_tx: &mpsc::UnboundedSender<TurnFinish>,
) -> Result<(), KernelError> {
    let cancel = Arc::new(Notify::new());
    let cancelled = Arc::new(AtomicBool::new(false));
    let inject_text = spawn.inject.as_ref().map(|item| item.text.clone());
    let mut begin = vec![NewEvent::new(
        state.session,
        EventKind::TurnStart,
        EventPayload::TurnStart {
            v: v1(),
            turn_id: spawn.turn,
            lane: LaneId::Dialogue.as_str(),
            origin: spawn.origin,
            delegation_id: None,
            trigger: TurnTrigger::Text,
        },
    )];
    if let Some(seq) = spawn.claim_seq {
        begin.push(NewEvent::new(
            state.session,
            EventKind::InboxClaimed,
            EventPayload::InboxClaimed {
                v: v1(),
                entry_seq: seq,
                turn_id: spawn.turn,
            },
        ));
    }
    if let Some(inject) = &spawn.inject {
        begin.push(NewEvent::new(
            state.session,
            EventKind::InboxClaimed,
            EventPayload::InboxClaimed {
                v: v1(),
                entry_seq: inject.seq,
                turn_id: spawn.turn,
            },
        ));
    }
    if let Some(note) = inject_text {
        begin.push(NewEvent::new(
            state.session,
            EventKind::ContextSystemMessage,
            EventPayload::ContextSystemMessage {
                v: v1(),
                blocks: vec![Block::text(note)],
                source_key: "steer.inject".to_owned(),
            },
        ));
    }
    for (key, text) in state.context.assemble() {
        begin.push(NewEvent::new(
            state.session,
            EventKind::ContextSystemMessage,
            EventPayload::ContextSystemMessage {
                v: v1(),
                blocks: vec![Block::text(text)],
                source_key: key,
            },
        ));
    }
    begin.push(NewEvent::new(
        state.session,
        EventKind::StepStart,
        EventPayload::StepStart {
            v: v1(),
            turn_id: spawn.turn,
            step_index: 0,
        },
    ));
    state
        .store
        .commit(Transaction {
            entries: begin,
            usage: Vec::new(),
        })
        .await?;
    state.context.set_interruption_note(None);
    let ctx = TurnCtx {
        store: Arc::clone(&state.store),
        session: state.session,
        soul: state.soul,
        turn: spawn.turn,
        model: Arc::clone(&state.model),
        observe: state.observe.clone(),
        live: state.live.clone(),
        window: state.mind.inner.self_reference_window,
        derive_from_thinking: state.mind.inner.derive_from_thinking,
        max_steps: state.max_steps,
        router: state.router.clone(),
        cancel: Arc::clone(&cancel),
        cancelled: Arc::clone(&cancelled),
        done: done_tx.clone(),
    };
    tokio::spawn(run_turn(ctx));
    state.running = Some(RunningTurn {
        turn: spawn.turn,
        cancel,
        cancelled,
    });
    Ok(())
}

fn request_abort(state: &mut LaneState) -> Result<(), KernelError> {
    let Some(running) = &state.running else {
        return Err(KernelError::no_active(&LaneId::Dialogue));
    };
    running.cancelled.store(true, Ordering::SeqCst);
    running.cancel.notify_waiters();
    Ok(())
}

async fn enqueue_wake(state: &mut LaneState, text: String) -> Result<u64, KernelError> {
    if state.running.is_none() {
        return Err(KernelError::no_active(&LaneId::Dialogue));
    }
    let text = validate_text(&text)?;
    let committed = state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::InboxEnqueued,
                EventPayload::InboxEnqueued {
                    v: v1(),
                    lane: LaneId::Dialogue.as_str(),
                    class: InboxClass::Wake,
                    source: InboxSource::User,
                    ref_seq: None,
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    let seq = committed.seqs.first().copied().unwrap_or(0);
    state.queued_wake = Some(QueuedWake { seq, text });
    Ok(seq)
}

async fn enqueue_inject(state: &mut LaneState, text: String) -> Result<u64, KernelError> {
    if state.running.is_none() {
        return Err(KernelError::no_active(&LaneId::Dialogue));
    }
    let text = validate_text(&text)?;
    let committed = state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::InboxEnqueued,
                EventPayload::InboxEnqueued {
                    v: v1(),
                    lane: LaneId::Dialogue.as_str(),
                    class: InboxClass::Inject,
                    source: InboxSource::Steer,
                    ref_seq: None,
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    let seq = committed.seqs.first().copied().unwrap_or(0);
    state.queued_inject = Some(QueuedInject { seq, text });
    Ok(seq)
}

async fn cancel_queued(state: &mut LaneState, entry_id: u64) -> Result<CancelQueued, KernelError> {
    if state.consumed_queue.contains(&entry_id) {
        return Ok(CancelQueued::AlreadyConsumed);
    }
    let wake_hit = state
        .queued_wake
        .as_ref()
        .is_some_and(|item| item.seq == entry_id);
    let inject_hit = state
        .queued_inject
        .as_ref()
        .is_some_and(|item| item.seq == entry_id);
    if !wake_hit && !inject_hit {
        return Ok(CancelQueued::NotFound);
    }
    state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::InboxCancelled,
                EventPayload::InboxCancelled {
                    v: v1(),
                    entry_seq: entry_id,
                    reason: ene_session::InboxCancelReason::User,
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    if wake_hit {
        state.queued_wake = None;
    }
    if inject_hit {
        state.queued_inject = None;
    }
    Ok(CancelQueued::Cancelled)
}

async fn run_compact(state: &LaneState) -> Result<u64, KernelError> {
    if let Some(running) = &state.running {
        return Err(KernelError::lane_busy(running.turn));
    }
    let events = state.store.load_events(state.session, 0)?;
    let from_seq = events
        .iter()
        .find(|event| {
            matches!(
                event.kind,
                EventKind::UserMessage | EventKind::AssistantMessage
            )
        })
        .map(|event| event.seq);
    let Some(from_seq) = from_seq else {
        return Err(KernelError::NothingToCompact);
    };
    let history = derive_messages(
        &events,
        ProjectOptions::for_depth(DisplayDepth::Detail, state.mind.inner.self_reference_window),
    );
    let summary = history
        .messages
        .iter()
        .map(ene_session::ProjectedMessage::text)
        .collect::<Vec<_>>()
        .join("\n");
    if summary.is_empty() {
        return Err(KernelError::NothingToCompact);
    }
    let truncated = truncate_chars(&summary, 2_000);
    let summary_commit = state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::SessionSummary,
                EventPayload::SessionSummary {
                    v: v1(),
                    scope: "compaction_ref".to_owned(),
                    summary: truncated,
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    let summary_seq = summary_commit.seqs.first().copied().unwrap_or(0);
    if from_seq >= summary_seq {
        return Err(KernelError::NothingToCompact);
    }
    state
        .store
        .commit(Transaction {
            entries: vec![NewEvent::new(
                state.session,
                EventKind::CompactionApplied,
                EventPayload::CompactionApplied {
                    v: v1(),
                    from_seq,
                    to_seq: summary_seq,
                    summary_event_seq: summary_seq,
                },
            )],
            usage: Vec::new(),
        })
        .await?;
    Ok(summary_seq)
}

struct TurnCtx {
    store: Arc<SessionStore>,
    session: SessionId,
    soul: SoulId,
    turn: TurnId,
    model: Arc<dyn ConversationModel>,
    observe: ObserveHandle,
    live: LiveBus,
    window: u32,
    derive_from_thinking: bool,
    max_steps: u32,
    router: Option<Arc<dyn SurfaceRouter>>,
    cancel: Arc<Notify>,
    cancelled: Arc<AtomicBool>,
    done: mpsc::UnboundedSender<TurnFinish>,
}

struct TurnFinish {
    turn: TurnId,
}

async fn run_turn(ctx: TurnCtx) {
    let turn = ctx.turn;
    let done = ctx.done.clone();
    let result = run_turn_inner(ctx).await;
    match result {
        Ok(()) => {
            drop(done.send(TurnFinish { turn }));
        }
        Err(err) => {
            warn!(error = %err, %turn, "turn failed");
            drop(done.send(TurnFinish { turn }));
        }
    }
}

async fn run_turn_inner(ctx: TurnCtx) -> Result<(), KernelError> {
    let span = ctx
        .observe
        .start("turn")
        .attr("turn_id", ctx.turn.to_string());
    debug_assert!(ctx.max_steps >= 1);
    let mut step_index = 0_u32;
    loop {
        if ctx.cancelled.load(Ordering::SeqCst) {
            finish_interrupted(&ctx, step_index).await?;
            span.end();
            return Ok(());
        }
        if step_index >= ctx.max_steps.max(1) {
            finish_speech(
                &ctx,
                &ModelGeneration {
                    text: "I'll look into that.".to_owned(),
                    finish_reason: "delegated".to_owned(),
                    ..ModelGeneration::default()
                },
                step_index.saturating_sub(1),
                None,
            )
            .await?;
            span.end();
            return Ok(());
        }

        let events = ctx.store.load_events(ctx.session, 0)?;
        let history = derive_messages(&events, ProjectOptions::model_visible(ctx.window));
        let request = ModelRequest {
            messages: history.messages.clone(),
        };
        let logged_hash = hash_projected(&history)?;
        let request_hash = hash_model_visible(&request.messages)?;
        debug_assert_eq!(
            logged_hash, request_hash,
            "model-visible must equal logged projection (L-1)"
        );

        let generation = tokio::select! {
            () = ctx.cancel.notified() => {
                finish_interrupted(&ctx, step_index).await?;
                span.end();
                return Ok(());
            }
            result = ctx.model.generate(request) => result?,
        };

        if ctx.cancelled.load(Ordering::SeqCst) {
            finish_interrupted(&ctx, step_index).await?;
            span.end();
            return Ok(());
        }

        let Some(call) = generation.tool_calls.first() else {
            finish_speech(&ctx, &generation, step_index, None).await?;
            span.end();
            return Ok(());
        };
        let Some(router) = ctx.router.as_ref() else {
            finish_speech(&ctx, &generation, step_index, None).await?;
            span.end();
            return Ok(());
        };

        match router
            .on_tool(&call.name, call.arguments.clone(), step_index)
            .await?
        {
            SurfaceToolOutcome::Delegated { speech, job_id } => {
                let generation = ModelGeneration {
                    text: speech,
                    finish_reason: "delegated".to_owned(),
                    ..generation
                };
                finish_speech(&ctx, &generation, step_index, Some(job_id)).await?;
                span.end();
                return Ok(());
            }
            SurfaceToolOutcome::Result(value) => {
                commit_tool_step(&ctx, &call.name, &call.arguments, value, step_index).await?;
                step_index = step_index.saturating_add(1);
            }
        }
    }
}

async fn finish_speech(
    ctx: &TurnCtx,
    generation: &ModelGeneration,
    step_index: u32,
    delegated_job: Option<String>,
) -> Result<(), KernelError> {
    let (speech, mut inner) = split_surface_and_inner(&generation.text);
    inner.extend(generation.inner.iter().cloned());
    if let Some(derived) = derive_thought_from_thinking(
        &inner,
        generation.thinking.as_deref(),
        ctx.derive_from_thinking,
    ) {
        inner.push(derived);
    }
    let mut end_entries = Vec::new();
    if let Some(job_id) = delegated_job.as_deref()
        && let Ok(delegation_id) = DelegationId::from_str(job_id)
    {
        end_entries.push(NewEvent::new(
            ctx.session,
            EventKind::DelegationStart,
            EventPayload::DelegationStart {
                v: v1(),
                delegation_id,
                mode: "public".to_owned(),
                goal_excerpt: truncate_chars(&generation.text, 120),
                budget: serde_json::json!({}),
            },
        ));
    }
    if let Some(thinking) = generation.thinking.clone() {
        end_entries.push(NewEvent::new(
            ctx.session,
            EventKind::AssistantThinking,
            EventPayload::AssistantThinking {
                v: v1(),
                turn_id: ctx.turn,
                step_index,
                blocks: vec![Block::text(thinking)],
                model_id: generation.model_id.clone(),
            },
        ));
    }
    let speech_for_live = if speech.is_empty() {
        generation.text.clone()
    } else {
        speech.clone()
    };
    for (aspect, text) in &inner {
        end_entries.push(NewEvent::new(
            ctx.session,
            EventKind::InnerMessage,
            EventPayload::InnerMessage {
                v: v1(),
                turn_id: Some(ctx.turn),
                step_index: Some(step_index),
                aspects: vec![*aspect],
                blocks: vec![Block::text(text)],
                model_visible: model_visible_for(*aspect),
            },
        ));
    }
    end_entries.push(NewEvent::new(
        ctx.session,
        EventKind::AssistantMessage,
        EventPayload::AssistantMessage {
            v: v1(),
            turn_id: ctx.turn,
            step_index,
            blocks: vec![Block::text(&speech_for_live)],
            finish_reason: generation.finish_reason.clone(),
            token_count: Some(generation.output_tokens),
        },
    ));
    end_entries.push(NewEvent::new(
        ctx.session,
        EventKind::StepEnd,
        EventPayload::StepEnd {
            v: v1(),
            turn_id: ctx.turn,
            step_index,
            outcome: StepOutcome::Stop,
            finish_reason: Some(generation.finish_reason.clone()),
        },
    ));
    end_entries.push(NewEvent::new(
        ctx.session,
        EventKind::TurnEnd,
        EventPayload::TurnEnd {
            v: v1(),
            turn_id: ctx.turn,
            outcome: TurnOutcome::Completed,
            error_class: None,
        },
    ));
    let usage = NewUsage {
        session_id: ctx.session,
        soul_id: ctx.soul,
        lane: LaneId::Dialogue.as_str(),
        task: "chat".to_owned(),
        provider: "echo".to_owned(),
        model: generation.model_id.clone(),
        entry_seq: None,
        input_tokens: generation.input_tokens,
        output_tokens: generation.output_tokens,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        cost_micro_usd: None,
        adjustment: false,
    };
    ctx.store
        .commit(Transaction {
            entries: end_entries,
            usage: vec![usage],
        })
        .await?;

    ctx.live.emit(
        DisplayDepth::Surface,
        LiveEvent::TextDelta {
            turn_id: ctx.turn,
            text: speech_for_live,
        },
    );
    if let Some((_, text)) = inner.first() {
        ctx.live.emit(
            DisplayDepth::Detail,
            LiveEvent::InnerMessage {
                turn_id: Some(ctx.turn),
                text: text.clone(),
            },
        );
    }
    if let Some(thinking) = generation.thinking.clone() {
        ctx.live.emit(
            DisplayDepth::Detail,
            LiveEvent::ThinkingDelta {
                turn_id: ctx.turn,
                text: thinking,
            },
        );
    }
    ctx.live.emit(
        DisplayDepth::Surface,
        LiveEvent::TurnEnded {
            turn_id: ctx.turn,
            outcome: "completed".to_owned(),
        },
    );
    Ok(())
}

async fn commit_tool_step(
    ctx: &TurnCtx,
    name: &str,
    args: &serde_json::Value,
    value: serde_json::Value,
    step_index: u32,
) -> Result<(), KernelError> {
    let call_id = CallId::new();
    let mut entries = vec![
        NewEvent::new(
            ctx.session,
            EventKind::ToolCall,
            EventPayload::ToolCall {
                v: v1(),
                turn_id: ctx.turn,
                step_index,
                call_id,
                tool_name: name.to_owned(),
                source: "surface".to_owned(),
                args: args.clone(),
            },
        ),
        NewEvent::new(
            ctx.session,
            EventKind::ToolResult,
            EventPayload::ToolResult {
                v: v1(),
                call_id,
                status: ToolStatus::Ok,
                blocks: vec![Block::text(value.to_string())],
                spill_ref: None,
                error_class: None,
                duration_ms: 0,
            },
        ),
        NewEvent::new(
            ctx.session,
            EventKind::StepEnd,
            EventPayload::StepEnd {
                v: v1(),
                turn_id: ctx.turn,
                step_index,
                outcome: StepOutcome::Next,
                finish_reason: Some("tool".to_owned()),
            },
        ),
    ];
    let next = step_index.saturating_add(1);
    if next < ctx.max_steps.max(1) {
        entries.push(NewEvent::new(
            ctx.session,
            EventKind::StepStart,
            EventPayload::StepStart {
                v: v1(),
                turn_id: ctx.turn,
                step_index: next,
            },
        ));
    }
    ctx.store
        .commit(Transaction {
            entries,
            usage: Vec::new(),
        })
        .await?;
    Ok(())
}

async fn finish_interrupted(ctx: &TurnCtx, step_index: u32) -> Result<(), KernelError> {
    ctx.store
        .commit(Transaction {
            entries: vec![
                NewEvent::new(
                    ctx.session,
                    EventKind::StepEnd,
                    EventPayload::StepEnd {
                        v: v1(),
                        turn_id: ctx.turn,
                        step_index,
                        outcome: StepOutcome::Error,
                        finish_reason: Some("interrupted".to_owned()),
                    },
                ),
                NewEvent::new(
                    ctx.session,
                    EventKind::TurnEnd,
                    EventPayload::TurnEnd {
                        v: v1(),
                        turn_id: ctx.turn,
                        outcome: TurnOutcome::Interrupted,
                        error_class: None,
                    },
                ),
            ],
            usage: Vec::new(),
        })
        .await?;
    ctx.live.emit(
        DisplayDepth::Surface,
        LiveEvent::TurnEnded {
            turn_id: ctx.turn,
            outcome: "interrupted".to_owned(),
        },
    );
    Ok(())
}

fn validate_text(text: &str) -> Result<String, KernelError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(KernelError::InvalidMessage(
            "message must not be empty".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    text.chars().take(max_chars).collect()
}
