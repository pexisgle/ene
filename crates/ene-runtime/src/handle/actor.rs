//! `TurnActor`: the single-threaded actor that owns turn-execution state.
//!
//! ## What runs here
//!
//! Turn execution (`Run` / `Cancel`), permission decisions, user-input
//! responses, snapshot, manual compression/undo, tool calls, character-card
//! swap, feature/proactive settings updates, tool-index invalidation, `CCv3`
//! memory hash, and plugin host restart all go through this single actor.
//!
//! Read-only session queries and pending-candidate approval never touch
//! `active_turn` / `stream_handle` / `turn_gate`, so they are served by
//! [`crate::query::sessions::SessionQueryHandle`] and
//! [`crate::query::candidates::MemoryCandidateHandle`] talking to
//! `MemoryStore` directly instead of queuing behind this mailbox.
//! Screen-image vision summarization runs its model call outside the actor
//! in [`crate::vision::VisionHandle`]; this actor only answers a small
//! `PrepareVisionSummary` request (busy-check + lazy model handle) and a
//! fire-and-forget `StashProactiveScreenImage`.
//!
//! Config/control operations (`SetCharacter`, `ApplySettings`,
//! plugin host restart) stay on this actor rather than a separate control
//! plane: they are infrequent, low-latency, and never block behind a `Run`
//! turn any worse than `Run` itself already does.

use super::SharedActorState;
use super::TurnGate;
use super::command::{DeferredToolTask, EneCommand};
use super::event::{
    AudioChunk, EneEvent, EneStateSnapshot, EneStatus, LifecycleEvent, MemoryLedgerChange,
    TerminalReason,
};
use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::error::EneRuntimeError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::{RequestId, TurnId};
use crate::vision::VisionPrepared;
use ene_ai::{AiTaskKind, ProviderHost, create_task_chat_provider};
use ene_config::EneConfig;
use ene_core::{ScheduleConfirmation, ScheduleRunStatus};
use ene_mind::commitments::CommitmentLedger;
use ene_mind::{CardName, SessionId};
use ene_mind::{
    CompressionLevel, CompressionResult, CompressionTaskInput, HistoryEntry as MindHistoryEntry,
    compression_has_usable_summary,
};
use ene_mind::{ConversationSession, EneSessionError};
use ene_mind::{
    GateRejectReason, QuietHoursPolicy, build_proactive_context, evaluate_deterministic_gates,
    evaluate_quiet_hours,
};
use ene_plugin_host::{
    CompositeToolRegistry, DisabledReason, PluginHealthEvent, PluginHostError, ToolRegistry,
};
use ene_rag::{ToolRag, ToolRagConfig, ToolRagOptions};
#[cfg(any(unix, windows))]
use ene_store::host_service::{DbPluginRegistration, HostServiceServer, host_service_socket_path};
use once_cell::sync::OnceCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Wall-clock source for quiet-hours evaluation; injectable so tests pin
/// deterministic instants.
type QuietHoursClock = Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>;

/// Global monotonic counter used to generate unique DB IPC auth tokens.
/// Intentionally process-global: each `EneHandle::open` call increments
/// the counter so concurrent handles never share a token.
static DB_TOKEN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) struct TurnActor {
    cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
    /// Clone of the command sender so background tasks spawned by the actor
    /// can send internal commands (e.g. [`EneCommand::PluginHostReconfigured`])
    /// back through the mailbox.
    cmd_tx: mpsc::UnboundedSender<EneCommand>,
    event_tx: broadcast::Sender<EneEvent>,
    /// Lifecycle bus sender: `StatusChanged` / `PendingCandidateAvailable`
    /// / `ToolBackgroundCompleted` — kept off the chat `event_tx` broadcast.
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    /// Audio channel sender: bounded `mpsc`, cloned into each stream
    /// task's [`crate::streaming::StreamContext`] so TTS chunks never ride
    /// the chat broadcast bus.
    audio_tx: mpsc::Sender<AudioChunk>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
    turn_gate: Arc<TurnGate>,
    config: EneConfig,
    /// Monotonic revision bumped on every successful unified settings apply.
    /// Drafts carry the value they were based on
    /// ([`SettingsApplyRequest::base_revision`]) so a stale writer is
    /// rejected instead of silently overwriting newer settings.
    settings_revision: u64,
    session: ConversationSession,
    /// Concrete store for IPC server (`connection()`) and `ToolRag`.
    ///
    /// `session.memory.memory_store` holds the same store as `Arc<dyn MemoryPort>`
    /// for `ene-mind`; this field keeps the concrete type for callers that need
    /// `MemoryStore`-specific methods not on the trait.
    concrete_store: Option<Arc<ene_store::MemoryStore>>,
    registry: Arc<dyn ToolRegistry>,
    tool_rag: Option<Arc<ToolRag>>,
    workspace_indexer: Option<Arc<crate::workspace::WorkspaceIndexer>>,
    workspace_state: Arc<parking_lot::Mutex<crate::workspace::WorkspaceActorState>>,
    workspace_sync_task: Option<tokio::task::JoinHandle<()>>,
    workspace_cancel: Option<CancellationToken>,
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<streaming::StreamOutcome>>,
    /// Shared with the running stream task; accumulates streamed assistant
    /// text deltas so a hard-aborted turn can still recover its partial
    /// response for interruption recording.
    stream_partial_text: Arc<parking_lot::Mutex<String>>,
    active_turn: Option<TurnId>,
    /// Cancellation token for the in-flight [`crate::vision::VisionHandle`]
    /// inference, if any. A fresh token is minted and handed out with each
    /// [`VisionPrepared`] reply; starting a new user turn cancels the
    /// current token and replaces it with a fresh one so a later vision call
    /// is not pre-cancelled. The actual inference call runs entirely outside
    /// this actor — this token is the only thread the actor still
    /// holds into an in-flight vision request, used solely to ask it to
    /// stop (it flows into `ene_infer::JobContext::should_stop` on the
    /// local llama.cpp worker).
    vision_cancel: CancellationToken,
    pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    /// Schedule runs waiting for a user confirmation decision, keyed by the
    /// `request_id` carried in the emitted `PermissionRequired` event.
    pending_schedule_confirmations: HashMap<RequestId, PendingScheduleConfirmation>,
    /// The schedule run currently executing (prompt stream or tool task).
    active_scheduled_run: Option<ActiveScheduledRun>,
    /// Wakes the scheduler timer task after any schedule-state mutation.
    pub(super) scheduler_notify: watch::Sender<()>,
    /// Receiver handed to the timer task when it is spawned.
    scheduler_notify_rx: Option<watch::Receiver<()>>,
    /// Wall-clock source for scheduler due-time evaluation (injectable in
    /// tests so scheduler integration tests advance virtual time).
    scheduler_clock: crate::scheduler::SchedulerClock,
    permission_scopes: Arc<Mutex<Vec<crate::streaming::PermissionScope>>>,
    /// Connector framework registry shared with the handle; the actor
    /// resolves permission prompts and records audit rows for lifecycle ops.
    connectors: Arc<ene_connector::ConnectorRegistry>,
    undo_stack: Arc<Mutex<crate::undo::UndoStack>>,
    context: ene_mind::ContextManager,
    call_tool_tasks: tokio::task::JoinSet<()>,
    classifier_tasks: tokio::task::JoinSet<()>,
    memory_writer_tasks: tokio::task::JoinSet<()>,
    /// Limits how many memory-writer outcome consumers run at once. Waiting
    /// consumers remain in [`Self::memory_writer_tasks`] so shutdown can abort
    /// them; `memory_writer_cap` is the permit count (0 disables admits and
    /// takes the short-lived reject+consume path).
    memory_writer_sem: Arc<Semaphore>,
    /// In-flight tool-search jobs; reaped so panics are not lost.
    search_tasks: tokio::task::JoinSet<()>,
    /// In-flight deferred (background) tool tasks. Each task polls
    /// its owning tool until the task reaches a terminal state, then emits
    /// [`LifecycleEvent::ToolBackgroundCompleted`]. Reaped so panics are not lost.
    deferred_tool_tasks: tokio::task::JoinSet<()>,
    /// Heavy command-handler work spawned to avoid head-of-line blocking:
    /// GGUF model loads, plugin host restarts. Reaped alongside the
    /// other `JoinSet`s so panics surface as diagnostics.
    bg_command_tasks: tokio::task::JoinSet<()>,
    /// Auxiliary stream-task handles (e.g. the TTS synthesis worker) handed
    /// back by the stream so shutdown can abort them. Each handle is
    /// wrapped in an [`AbortOnDrop`] guard inside a wrapper task, so aborting
    /// the set (or dropping it on teardown) aborts the underlying worker too
    /// — they cannot outlive the actor. Reaped like the other `JoinSet`s.
    aux_tasks: tokio::task::JoinSet<()>,
    classifier_rx: mpsc::UnboundedReceiver<tokio::task::JoinHandle<()>>,
    memory_writer_rx:
        mpsc::UnboundedReceiver<tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>>,
    deferred_tool_rx: mpsc::UnboundedReceiver<DeferredToolTask>,
    /// Receiver for auxiliary stream-task handles (e.g. the TTS worker).
    /// Drained by [`TurnActor::admit_aux_handles`] from the run loop, the
    /// `Shutdown` command handler, and the post-loop teardown so a handle
    /// that arrives after the loop's last drain is still admitted and aborted
    /// on shutdown rather than dropped (which would only detach, not cancel,
    /// the worker).
    aux_task_rx: mpsc::UnboundedReceiver<tokio::task::JoinHandle<()>>,
    classifier_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    memory_writer_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>>,
    deferred_tool_tx: mpsc::UnboundedSender<DeferredToolTask>,
    aux_task_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    /// Shared with the running stream task; first party to flip emits Terminal.
    terminal_emitted: Arc<AtomicBool>,
    /// Held so the broadcast channel retains buffered diagnostic events until the
    /// first subscriber attaches via [`crate::EneHandle::diagnostics().subscribe()`].
    _diag_rx: broadcast::Receiver<DiagnosticEvent>,
    proactive: crate::proactive::ProactiveScheduler,
    proactive_decision_rx: Option<oneshot::Receiver<crate::proactive::ProactiveDecisionResult>>,
    proactive_decision_handle: Option<tokio::task::JoinHandle<()>>,
    proactive_resolution_rx: Option<oneshot::Receiver<crate::proactive::PendingResolutionResult>>,
    proactive_resolution_handle: Option<tokio::task::JoinHandle<()>>,
    /// Wall-clock source for quiet-hours evaluation (injectable in tests).
    quiet_hours_clock: QuietHoursClock,
    /// True while a proactive turn runs with notifications suppressed, so the
    /// matching `Idle` announcement is suppressed too.
    quiet_hours_notifications_suppressed: bool,
    /// Local / cloud decision provider handles (lazy).
    ///
    /// Shared via `Arc` so background init tasks can populate it without
    /// blocking the actor loop. The `OnceCell` guarantees at-most-once
    /// initialization; `proactive_llm_init_spawned` prevents duplicate
    /// background spawns.
    proactive_llm: Arc<OnceCell<crate::proactive_llm::ProactiveLlmHandles>>,
    /// Guards against spawning multiple background init tasks for
    /// [`Self::proactive_llm`]. Set to `true` when a background load
    /// is spawned; reset to `false` if that load *fails* so a later call can
    /// retry — a single transient failure must not permanently disable
    /// proactive features for the process lifetime. On success the `OnceCell`
    /// is populated, so the `get().is_some()` fast-path short-circuits before
    /// this flag is consulted again. Shared via `Arc` so the background init
    /// task can reset it on failure.
    proactive_llm_init_spawned: Arc<AtomicBool>,
    /// Guards against spawning duplicate `PrepareVisionSummary` slow-path
    /// pollers. The screen-capture-driven proactive vision flow can
    /// fire `PrepareVisionSummary` several times during the multi-second GGUF
    /// loading window; without deduplication each call would spawn a fresh
    /// up-to-five-minute poller into `bg_command_tasks` (default cap 4),
    /// exhausting the cap and starving unrelated background work (e.g. a
    /// concurrent `PluginHostReconfigure`). Set while a poller is in flight;
    /// the poller clears it just before exiting. Shared via `Arc` so the
    /// background poller can clear it.
    vision_slow_path_active: Arc<AtomicBool>,
    /// Origin of the active stream turn (for cancel Terminal).
    active_origin: crate::types::TurnOrigin,
    health_monitor: ene_ai::ProviderHealthMonitor,
    /// Bounded-task admission caps for the five `JoinSet`s above, plus the
    /// deferred (background) tool poll budget. Loaded once at
    /// construction from the `tools.*` config section (see
    /// [`crate::task_config::ToolRuntimeConfig`]); a config hot-reload does
    /// not currently re-read it (matching `deferred_max_polls`'s
    /// once-at-startup behavior).
    task_caps: crate::task_config::ToolRuntimeConfig,
    tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
    /// Plugin-contributed tool registries, re-merged when the tool registry is
    /// rebuilt after a Features update.
    plugin_tool_registries: Vec<Arc<dyn ToolRegistry>>,
    /// Live provider catalog: routes provider creation to the current plugin
    /// host. Kept separate from [`Self::plugin_host`] so test stubs can
    /// stand in for the host and so lazy resolvers (the embedding proxy)
    /// share one seam.
    provider_host: Arc<dyn ProviderHost>,
    /// Shared plugin host manager handle. Held by the actor so a Features
    /// update that changes the enabled plugin set can restart the host with
    /// the new configuration (E1). Shared with [`crate::EneHandle`] so shutdown
    /// tears down whichever host is currently live.
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    /// Shared handle to the plugin health → diagnostics bridge task. Kept in
    /// sync when the plugin host is restarted so shutdown aborts the live
    /// bridge rather than a stale one.
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Shared handle to the host-service accept loop. Kept in sync when the
    /// plugin host is restarted so reconfiguration aborts the live acceptor
    /// before the shared socket path is rebound.
    host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Mailbox-free shared actor state: card name (shared with
    /// [`crate::query::candidates::MemoryCandidateHandle`]),
    /// session id / started-at / turn count, config, and the loaded card.
    /// [`crate::EneHandle`] reads these slots synchronously; this actor keeps
    /// them in sync at the mutation points (session split, `SetCharacter`,
    /// per-turn bookkeeping, feature-settings updates).
    shared: Arc<SharedActorState>,
}

/// A schedule run waiting for a user confirmation decision.
struct PendingScheduleConfirmation {
    schedule_id: i64,
    run_id: i64,
    /// Aborted when the user answers, so the timeout task never fires a
    /// stale `ScheduleConfirmationTimeout` for an approved/denied run.
    timeout: CancellationToken,
}

/// The schedule run currently executing (prompt stream or tool task).
struct ActiveScheduledRun {
    schedule_id: i64,
    run_id: i64,
}

impl TurnActor {
    /// Constructs a ready `TurnActor`. Called once from [`crate::EneHandle::open`].
    #[expect(
        clippy::too_many_arguments,
        reason = "single internal constructor call site (EneHandle::open); grouping the \
                  bootstrap channels/handles into a config struct is a larger refactor \
                  out of scope here"
    )]
    pub(super) fn new(
        cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
        cmd_tx: mpsc::UnboundedSender<EneCommand>,
        event_tx: broadcast::Sender<EneEvent>,
        lifecycle_tx: broadcast::Sender<LifecycleEvent>,
        audio_tx: mpsc::Sender<AudioChunk>,
        diag_tx: broadcast::Sender<DiagnosticEvent>,
        diag_rx: broadcast::Receiver<DiagnosticEvent>,
        turn_gate: Arc<TurnGate>,
        config: EneConfig,
        session: ConversationSession,
        concrete_store: Option<Arc<ene_store::MemoryStore>>,
        registry: Arc<dyn ToolRegistry>,
        tool_rag: Option<Arc<ToolRag>>,
        workspace_indexer: Option<Arc<crate::workspace::WorkspaceIndexer>>,
        health_monitor: ene_ai::ProviderHealthMonitor,
        tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
        plugin_tool_registries: Vec<Arc<dyn ToolRegistry>>,
        provider_host: Arc<dyn ProviderHost>,
        plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
        health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        shared_state: Arc<SharedActorState>,
        connectors: Arc<ene_connector::ConnectorRegistry>,
        scheduler_clock: crate::scheduler::SchedulerClock,
    ) -> Self {
        let (classifier_tx, classifier_rx) = mpsc::unbounded_channel();
        let (memory_writer_tx, memory_writer_rx) = mpsc::unbounded_channel();
        let (deferred_tool_tx, deferred_tool_rx) = mpsc::unbounded_channel();
        let (aux_task_tx, aux_task_rx) = mpsc::unbounded_channel();
        let (scheduler_notify, scheduler_notify_rx) = watch::channel(());
        let task_caps = config
            .get_section::<crate::task_config::ToolRuntimeConfig>()
            .unwrap_or_default();
        let memory_writer_sem = Arc::new(Semaphore::new(task_caps.memory_writer_cap));
        Self {
            cmd_rx,
            cmd_tx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            diag_tx,
            turn_gate,
            config,
            settings_revision: 0,
            session,
            concrete_store,
            registry,
            tool_rag,
            workspace_indexer,
            workspace_state: Arc::new(parking_lot::Mutex::new(
                crate::workspace::WorkspaceActorState::default(),
            )),
            workspace_sync_task: None,
            workspace_cancel: None,
            cancel_token: CancellationToken::new(),
            stream_handle: None,
            stream_session_rx: None,
            stream_partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
            active_turn: None,
            vision_cancel: CancellationToken::new(),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_schedule_confirmations: HashMap::new(),
            active_scheduled_run: None,
            scheduler_notify,
            scheduler_notify_rx: Some(scheduler_notify_rx),
            scheduler_clock,
            permission_scopes: Arc::new(Mutex::new(Vec::new())),
            connectors,
            undo_stack: Arc::new(Mutex::new(crate::undo::UndoStack::new(64))),
            context: ene_mind::ContextManager::default(),
            call_tool_tasks: tokio::task::JoinSet::new(),
            classifier_tasks: tokio::task::JoinSet::new(),
            memory_writer_tasks: tokio::task::JoinSet::new(),
            memory_writer_sem,
            search_tasks: tokio::task::JoinSet::new(),
            deferred_tool_tasks: tokio::task::JoinSet::new(),
            bg_command_tasks: tokio::task::JoinSet::new(),
            aux_tasks: tokio::task::JoinSet::new(),
            classifier_rx,
            memory_writer_rx,
            deferred_tool_rx,
            aux_task_rx,
            terminal_emitted: Arc::new(AtomicBool::new(false)),
            classifier_tx,
            memory_writer_tx,
            deferred_tool_tx,
            aux_task_tx,
            _diag_rx: diag_rx,
            proactive: crate::proactive::ProactiveScheduler::default(),
            proactive_decision_rx: None,
            proactive_decision_handle: None,
            proactive_resolution_rx: None,
            proactive_resolution_handle: None,
            quiet_hours_clock: Arc::new(chrono::Utc::now),
            quiet_hours_notifications_suppressed: false,
            proactive_llm: Arc::new(OnceCell::new()),
            proactive_llm_init_spawned: Arc::new(AtomicBool::new(false)),
            vision_slow_path_active: Arc::new(AtomicBool::new(false)),
            active_origin: crate::types::TurnOrigin::User,
            health_monitor,
            task_caps,
            tts_provider,
            plugin_tool_registries,
            provider_host,
            plugin_host,
            health_bridge_handle,
            host_service_handle,
            shared: shared_state,
        }
    }

    /// Runs a single command through [`isolate_panic`] so a panicking
    /// command handler is contained instead of taking down the actor task.
    ///
    /// A panicking command is logged and surfaced as a [`DiagnosticEvent::ActorPanic`]
    /// instead of unwinding the whole actor task (which would take down the process).
    /// The command is treated as non-terminal so subsequent commands keep flowing.
    async fn run_command_isolated(&mut self, cmd: EneCommand) -> bool {
        let diag_tx = self.diag_tx.clone();
        isolate_panic(&diag_tx, "command", self.handle_command(cmd))
            .await
            .unwrap_or(true)
    }

    /// Drains auxiliary stream-task handles (e.g. the TTS worker) sent from
    /// the stream so shutdown can abort them.
    ///
    /// Each handle is wrapped in an [`AbortOnDrop`] guard inside a wrapper
    /// task: aborting the wrapper (via `aux_tasks.abort_all()`) drops the
    /// guard, which aborts the underlying worker instead of merely detaching
    /// it. Called from the run loop, the `Shutdown` command handler, and the
    /// post-loop teardown so a handle that arrives after the loop's last
    /// drain is still admitted and aborted on shutdown. No admission cap: at
    /// most a handful are spawned per turn and they are bounded by the turn's
    /// own lifecycle.
    fn admit_aux_handles(&mut self) {
        while let Ok(handle) = self.aux_task_rx.try_recv() {
            // Construct the guard *outside* the wrapper future so it becomes
            // part of the future's captured state: even if the wrapper is
            // aborted before its first poll (a shutdown right after the
            // handle was admitted), dropping the future drops the guard and
            // aborts the inner worker rather than merely detaching it.
            let mut worker = AbortOnDrop(handle);
            self.aux_tasks.spawn(async move {
                if let Err(e) = (&mut worker.0).await {
                    tracing::warn!(
                        component = "TurnActor",
                        error = %e,
                        "Auxiliary stream task failed"
                    );
                }
            });
        }
    }

    /// Drains memory-writer `JoinHandle`s sent from the stream into
    /// `memory_writer_tasks`, with concurrency limited by
    /// [`Self::memory_writer_sem`] (`tools.memory_writer_cap`).
    ///
    /// The memory write itself is already running detached (spawned by the
    /// stream task before its `JoinHandle` reached us); admission here only
    /// bounds outcome consumers. Consumers that wait for a semaphore permit
    /// stay in the supervised `JoinSet` so shutdown can abort them — there is
    /// no unbounded detached `tokio::spawn` overflow path.
    ///
    /// - `cap == 0`: emit [`DiagnosticEvent::TaskRejected`] and still spawn a
    ///   short-lived supervised consumer so lifecycle / diagnostic events are
    ///   not lost (the historical contract the Stage 8 tests rely on).
    /// - `cap > 0`: admit into the `JoinSet` up to
    ///   [`memory_writer_hard_limit`]; excess handles are hard-dropped (no
    ///   outcome consumer, `TaskRejected` already emitted by [`admit_task`]).
    ///   Admitted tasks acquire a semaphore permit before consuming so at most
    ///   `cap` consumers run concurrently.
    fn drain_memory_writers(&mut self) {
        while let Ok(handle) = self.memory_writer_rx.try_recv() {
            let diag_tx = self.diag_tx.clone();
            let lifecycle_tx = self.lifecycle_tx.clone();
            let store = self.concrete_store.clone();
            let cap = self.task_caps.memory_writer_cap;

            if cap == 0 {
                tracing::warn!(
                    component = "MemoryWriter",
                    cap,
                    "background task rejected: memory_writer_cap is 0"
                );
                emit_diag(
                    &self.diag_tx,
                    DiagnosticEvent::TaskRejected {
                        component: "MemoryWriter".to_string(),
                        cap,
                        detail: None,
                    },
                );
                self.memory_writer_tasks.spawn(async move {
                    consume_memory_write_outcome(handle, lifecycle_tx, diag_tx, store).await;
                });
                continue;
            }

            let hard_limit = memory_writer_hard_limit(cap);
            if !admit_task(
                &mut self.memory_writer_tasks,
                hard_limit,
                "MemoryWriter",
                None,
                &self.diag_tx,
            ) {
                // Hard drop: JoinSet waiter queue is full. The write itself
                // keeps running detached; only the outcome consumer is lost.
                drop(handle);
                continue;
            }

            let sem = Arc::clone(&self.memory_writer_sem);
            self.memory_writer_tasks.spawn(async move {
                let Ok(_permit) = sem.acquire().await else {
                    return;
                };
                consume_memory_write_outcome(handle, lifecycle_tx, diag_tx, store).await;
            });
        }
    }

    /// Test-only hook: injects a memory-writer `JoinHandle` as if the
    /// stream task had sent it, then runs [`Self::drain_memory_writers`] so
    /// admission-cap behavior can be exercised without a live stream task.
    #[cfg(test)]
    pub(super) fn inject_and_drain_memory_writer(
        &mut self,
        handle: tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>,
    ) {
        self.memory_writer_tx
            .send(handle)
            .expect("memory-writer channel is open in tests");
        self.drain_memory_writers();
    }

    /// Starts the background workspace index sync (single-flight).
    ///
    /// The sync task updates [`Self::workspace_state`] as it progresses and
    /// stores its report on completion; cancellation is signalled through
    /// [`Self::workspace_cancel`].
    fn spawn_workspace_sync(&mut self) -> Result<(), EneRuntimeError> {
        let Some(indexer) = self.workspace_indexer.clone() else {
            return Err(EneRuntimeError::MindPrerequisite(
                "workspace indexer unavailable",
            ));
        };
        if self.workspace_sync_task.is_some() {
            return Err(EneRuntimeError::Busy { queue_depth: 1 });
        }
        let config = self
            .config
            .get_section::<ene_rag::WorkspaceRagConfig>()
            .unwrap_or_default();
        if !config.enabled {
            return Err(crate::workspace::WorkspaceIndexError::Disabled.into());
        }
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let state = Arc::clone(&self.workspace_state);
        {
            let mut guard = state.lock();
            guard.in_progress = true;
            guard.progress = crate::workspace::WorkspaceSyncProgress::default();
            guard.last_error = None;
        }
        // Live progress: the indexer emits snapshots on an mpsc channel; a
        // forwarding task writes them into the shared state so
        // `/workspace status` shows the running sync, not a frozen zeroed
        // snapshot. The forwarder exits when the channel closes (the sync
        // drops its sender on completion).
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let forward_state = Arc::clone(&state);
        let forwarder = tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                forward_state.lock().progress = progress;
            }
        });
        let task = tokio::spawn(async move {
            let result = indexer.sync(&config, &task_cancel, Some(progress_tx)).await;
            // Drain the progress queue before writing the terminal state so
            // the final snapshot reflects every emitted event.
            #[expect(
                clippy::let_underscore_must_use,
                reason = "forwarder JoinHandle failure is unactionable; the channel already closed"
            )]
            let _ = forwarder.await;
            let mut guard = state.lock();
            guard.in_progress = false;
            match result {
                Ok(report) => {
                    guard.progress = crate::workspace::WorkspaceSyncProgress {
                        phase: crate::workspace::WorkspaceSyncPhase::Done,
                        ..guard.progress.clone()
                    };
                    guard.last_report = Some(report);
                }
                Err(e) => {
                    guard.last_error = Some(e.to_string());
                    tracing::warn!(
                        component = "WorkspaceRag",
                        error = %e,
                        "Workspace index sync failed"
                    );
                }
            }
        });
        self.workspace_sync_task = Some(task);
        self.workspace_cancel = Some(cancel);
        Ok(())
    }

    /// Re-embeds an edited memory's content in the background so vector
    /// recall does not serve stale text.
    ///
    /// Best-effort: the in-place edit has already committed and lexical recall
    /// is immediately correct, so embedding failures are logged, never
    /// surfaced to the caller. The task captures the row's `updated_at` before
    /// embedding and re-checks it right before writing; a newer edit (which
    /// spawns its own task) bumps `updated_at`, so a slow older task skips its
    /// write instead of overwriting the newer embedding.
    async fn spawn_memory_reembed(&mut self, id: i64, edit: &ene_store::MemoryEdit) {
        let Some(store) = self.concrete_store.clone() else {
            return;
        };
        let Some(embedder) = self.session.memory.embedding_provider.clone() else {
            return;
        };
        let Ok(Some(memory)) = store.get_typed_memory(id).await else {
            return;
        };
        let expected_updated_at = memory.updated_at;
        if !admit_task(
            &mut self.bg_command_tasks,
            self.task_caps.bg_command_cap,
            "BgCommand",
            Some("MemoryReembed".to_string()),
            &self.diag_tx,
        ) {
            tracing::warn!(
                component = "TurnActor",
                memory_id = id,
                "Memory re-embed rejected: background task capacity exhausted"
            );
            return;
        }

        let content = edit.content.clone();
        self.bg_command_tasks.spawn(async move {
            let model_name = embedder.model_name();
            let Ok(embedding) =
                ene_ai::embed(embedder.as_ref(), &content, ene_ai::EmbeddingKind::Summary).await
            else {
                tracing::warn!(
                    component = "MemoryReembed",
                    memory_id = id,
                    "Failed to embed edited memory content"
                );
                return;
            };
            let Ok(Some(current)) = store.get_typed_memory(id).await else {
                return;
            };
            if current.updated_at != expected_updated_at {
                tracing::debug!(
                    component = "MemoryReembed",
                    memory_id = id,
                    "Skipping stale re-embed; memory was edited again"
                );
                return;
            }
            if let Err(error) = store
                .upsert_memory_embedding(id, model_name, "content", &embedding)
                .await
            {
                tracing::warn!(
                    component = "MemoryReembed",
                    memory_id = id,
                    error = %error,
                    "Failed to re-embed edited memory content"
                );
            }
        });
    }

    pub(super) async fn run(mut self) {
        self.reconcile_scheduler_startup().await;
        self.spawn_scheduler_task();
        // Privacy-first startup sync: only when the operator explicitly
        // enabled the feature and asked for a startup pass.
        let startup_sync = self
            .config
            .get_section::<ene_rag::WorkspaceRagConfig>()
            .unwrap_or_default()
            .sync_on_startup;
        if startup_sync {
            match self.spawn_workspace_sync() {
                Ok(()) => {
                    tracing::info!(
                        component = "WorkspaceRag",
                        "Workspace index startup sync started"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        component = "WorkspaceRag",
                        error = %e,
                        "Workspace index startup sync skipped"
                    );
                }
            }
        }
        loop {
            // Reap completed background tasks so the JoinSets shrink again
            // once work finishes. Reaping alone only bounds steady-state
            // size, not the *rate* of admission during a burst — each
            // `JoinSet` also has an explicit cap enforced at its spawn
            // site(s) via `admit_task` (Stage 8), so a burst that arrives
            // faster than tasks complete is rejected rather than queued
            // without bound.
            reap_join_set(
                &mut self.call_tool_tasks,
                "CallToolReaper",
                "CallTool task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.classifier_tasks,
                "ClassifierReaper",
                "Classifier task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.memory_writer_tasks,
                "MemoryWriterReaper",
                "Deferred memory-writer task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.search_tasks,
                "SearchToolsReaper",
                "SearchTools task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.deferred_tool_tasks,
                "DeferredToolReaper",
                "Deferred tool task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.bg_command_tasks,
                "BgCommandReaper",
                "Background command task panicked",
                &self.diag_tx,
            );
            reap_join_set(
                &mut self.aux_tasks,
                "AuxTaskReaper",
                "Auxiliary stream task panicked",
                &self.diag_tx,
            );
            if let Some(handle) = self.workspace_sync_task.as_ref()
                && handle.is_finished()
            {
                self.workspace_sync_task = None;
                self.workspace_cancel = None;
            }

            self.admit_aux_handles();

            while let Ok(handle) = self.classifier_rx.try_recv() {
                if !admit_task(
                    &mut self.classifier_tasks,
                    self.task_caps.classifier_cap,
                    "Classifier",
                    None,
                    &self.diag_tx,
                ) {
                    // The classifier itself is already running detached
                    // (tokio::spawn'd by the stream task before its
                    // JoinHandle reached us); dropping `handle` here without
                    // awaiting it only stops *our* panic supervision of it —
                    // it does not cancel the task (quality loss, not a
                    // correctness bug).
                    continue;
                }
                self.classifier_tasks.spawn(async move {
                    if let Err(e) = handle.await {
                        tracing::error!(
                            component = "EmotionEngine",
                            error = %e,
                            "Post-turn affect classifier panicked"
                        );
                    }
                });
            }

            self.drain_memory_writers();

            while let Ok(task) = self.deferred_tool_rx.try_recv() {
                if !admit_task(
                    &mut self.deferred_tool_tasks,
                    self.task_caps.deferred_tool_cap,
                    "DeferredTool",
                    Some(format!("{}:{}", task.tool_name, task.task_id)),
                    &self.diag_tx,
                ) {
                    // Unlike the classifier/memory-writer drains, there is no
                    // detached task already running here: the underlying
                    // plugin-side tool call keeps executing regardless, but
                    // nothing will poll it to completion, so no
                    // `ToolBackgroundCompleted` lifecycle event will ever
                    // fire for this task. The `TaskRejected` diagnostic above
                    // is the only signal a consumer gets.
                    continue;
                }
                let registry = Arc::clone(&self.registry);
                let lifecycle_tx = self.lifecycle_tx.clone();
                let max_polls = self.task_caps.deferred_max_polls;
                self.deferred_tool_tasks.spawn(async move {
                    poll_deferred_task(registry, lifecycle_tx, task, max_polls).await;
                });
            }

            if let Some(rx) = self.stream_session_rx.as_mut() {
                // Poll the stream-completion receiver directly
                // alongside the command channel so we react to
                // a finished stream within the same event-loop
                // tick (< 10 ms) instead of waking every 100 ms
                // to re-check. The `oneshot::Receiver` is
                // `Unpin`, so a `&mut` borrow is enough to
                // keep it alive across `select!` arms; if the
                // command branch fires, the receiver is
                // re-polled on the next iteration.
                tokio::select! {
                    cmd = self.cmd_rx.recv() => {
                        match cmd {
                            Some(cmd) => {
                                if !self.run_command_isolated(cmd).await {
                                    break;
                                }
                            }
                            None => break, // All senders dropped
                        }
                    }
                    res = &mut *rx => {
                        if let Ok(outcome) = res {
                            self.session = outcome.session;
                            // The stream task recorded the assistant
                            // response on its session clone; publish the
                            // (possibly incremented) turn count.
                            self.sync_shared_session_state();
                            if self.active_origin == crate::types::TurnOrigin::Proactive
                                && let Some(decision) = self.proactive.last_decision.take()
                            {
                                let confirmation = crate::proactive::apply_proactive_completion(
                                    &mut self.proactive,
                                    &decision,
                                    &outcome.terminal,
                                    outcome.spoke_visible_text,
                                );
                                crate::proactive::log_confirmation(&decision, confirmation);
                            }
                            // A completed user turn is the reply to a
                            // delivered confirmation question when one is
                            // outstanding; classify it (fail-closed) and
                            // resolve the candidate through the approval
                            // APIs. Mid-stream failures and cancels still
                            // recorded the user's message at turn start, so
                            // they are classified too; only a provider-open
                            // failure (no stream, no outcome) defers to the
                            // next turn.
                            if self.active_origin == crate::types::TurnOrigin::User
                                && matches!(
                                    outcome.terminal,
                                    TerminalReason::Done
                                        | TerminalReason::Failed { .. }
                                        | TerminalReason::Cancelled
                                )
                                && self.proactive.asked_pending_candidate.is_some()
                            {
                                self.spawn_pending_resolution();
                            }
                            // A scheduled prompt run finished with this
                            // outcome; record it before the run state below
                            // is cleared.
                            if let Some(run) = self.active_scheduled_run.take() {
                                let (status, error) = match &outcome.terminal {
                                    TerminalReason::Done => {
                                        (ScheduleRunStatus::Success, None)
                                    }
                                    TerminalReason::Failed { message } => {
                                        (ScheduleRunStatus::Failed, Some(message.clone()))
                                    }
                                    TerminalReason::Cancelled => (
                                        ScheduleRunStatus::Failed,
                                        Some("cancelled".to_string()),
                                    ),
                                    TerminalReason::Declined => (
                                        ScheduleRunStatus::Failed,
                                        Some("declined".to_string()),
                                    ),
                                };
                                self.finish_scheduled_run(
                                    run.schedule_id,
                                    run.run_id,
                                    status,
                                    error,
                                )
                                .await;
                            }
                            // Retroactive topic-boundary compression: a
                            // boundary detected on the just-completed turn
                            // compresses the span before it. Runs here (after
                            // Terminal, in the actor) so the response is never
                            // delayed; the summary is applied at the start of
                            // the next turn via `apply_pending_compression`.
                            if let Some(score) = outcome.topic_boundary_score {
                                self.perform_retroactive_compression(score).await;
                            }
                        } else if self
                            .terminal_emitted
                            .compare_exchange(
                                false,
                                true,
                                std::sync::atomic::Ordering::AcqRel,
                                std::sync::atomic::Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            // Stream task ended without sending an outcome and
                            // without its own catch_unwind handler claiming
                            // `terminal_emitted` first (e.g. the sender was
                            // dropped before the handler ran). Emit a fallback
                            // Terminal so consumers do not wait forever.
                            if let Some(run) = self.active_scheduled_run.take() {
                                self.finish_scheduled_run(
                                    run.schedule_id,
                                    run.run_id,
                                    ScheduleRunStatus::Failed,
                                    Some("stream task terminated unexpectedly".to_string()),
                                )
                                .await;
                            }
                            drop(self.event_tx.send(EneEvent::Terminal {
                                turn: self.active_turn.clone().unwrap_or_default(),
                                origin: self.active_origin,
                                reason: TerminalReason::Failed {
                                    message: "stream task terminated unexpectedly".into(),
                                },
                            }));
                        }
                        self.stream_handle = None;
                        self.stream_session_rx = None;
                        self.active_turn = None;
                        self.active_origin = crate::types::TurnOrigin::User;
                        self.turn_gate.end();
                        if !self.quiet_hours_notifications_suppressed {
                            drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                                status: EneStatus::Idle,
                            }));
                        }
                        self.quiet_hours_notifications_suppressed = false;
                    }
                }
            } else {
                let mind = self
                    .config
                    .get_section::<ene_mind::MindConfig>()
                    .unwrap_or_default();
                let proactive_enabled = mind.proactive.enabled;
                let tick = crate::proactive::tick_period(&mind.proactive);

                if let Some(rx) = self.proactive_decision_rx.as_mut() {
                    tokio::select! {
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(cmd) => {
                                    if !self.run_command_isolated(cmd).await {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                        decision = &mut *rx => {
                            self.proactive_decision_rx = None;
                            self.proactive_decision_handle = None;
                            if let Ok(result) = decision {
                                self.handle_proactive_decision(result).await;
                            }
                        }
                    }
                } else if let Some(rx) = self.proactive_resolution_rx.as_mut() {
                    tokio::select! {
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(cmd) => {
                                    if !self.run_command_isolated(cmd).await {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                        resolution = &mut *rx => {
                            self.proactive_resolution_rx = None;
                            self.proactive_resolution_handle = None;
                            if let Ok(result) = resolution {
                                self.handle_proactive_resolution(result).await;
                            }
                        }
                    }
                } else if proactive_enabled {
                    tokio::select! {
                        cmd = self.cmd_rx.recv() => {
                            match cmd {
                                Some(cmd) => {
                                    if !self.run_command_isolated(cmd).await {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                        () = tokio::time::sleep(tick) => {
                            // When screen_summary is on, decisions are driven by
                            // fresh observation pushes (capture → vision → decide).
                            let screen_driven = mind.proactive.sources.screen_summary;
                            if !screen_driven {
                                self.maybe_spawn_proactive_decision().await;
                            }
                        }
                    }
                } else {
                    match self.cmd_rx.recv().await {
                        Some(cmd) => {
                            if !self.run_command_isolated(cmd).await {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }

        if let Some(handle) = self.stream_handle.take() {
            handle.abort();
        }
        // Cooperative cancel first: aux workers watching the turn's token
        // (the TTS pipeline) exit gracefully instead of being force-aborted.
        self.cancel_token.cancel();
        // Admit aux handles that arrived after the run loop's last drain so
        // teardown aborts them rather than dropping them (which would only
        // detach, not cancel, the worker).
        self.admit_aux_handles();
        self.abort_all_join_sets();
        self.abort_proactive_resolution();
        self.drain_pending().await;
    }

    /// Aborts every background `JoinSet` the actor supervises.
    ///
    /// Shared by post-loop teardown and [`EneCommand::Shutdown`] so both paths
    /// abort the same sets (including `search_tasks` and `deferred_tool_tasks`)
    /// in the same order. Callers that need aux handles admitted first must
    /// invoke [`Self::admit_aux_handles`] before this.
    fn abort_all_join_sets(&mut self) {
        self.classifier_tasks.abort_all();
        self.memory_writer_tasks.abort_all();
        self.call_tool_tasks.abort_all();
        self.search_tasks.abort_all();
        self.deferred_tool_tasks.abort_all();
        self.bg_command_tasks.abort_all();
        self.aux_tasks.abort_all();
    }

    /// Drops every pending permission and user-input
    /// `oneshot::Sender`, releasing the receiver futures that
    /// were awaiting them. Called on `Run` (to clear entries
    /// left by the previous run after a cancel), `Cancel`,
    /// and `Shutdown` so the maps do not grow unboundedly
    /// across cancel-during-prompt cycles.
    async fn drain_pending(&self) {
        let mut guard = self.pending_permissions.lock().await;
        guard.clear();
        drop(guard);
        let mut guard = self.pending_user_inputs.lock().await;
        guard.clear();
    }

    fn abort_proactive_decision(&mut self) {
        if let Some(handle) = self.proactive_decision_handle.take() {
            handle.abort();
        }
        self.proactive_decision_rx = None;
    }

    /// Drop an in-flight reply classification. Called on character/session
    /// reset and shutdown; deliberately *not* on user-turn start, so a reply
    /// already under classification is not lost when the user types again.
    fn abort_proactive_resolution(&mut self) {
        if let Some(handle) = self.proactive_resolution_handle.take() {
            handle.abort();
        }
        self.proactive_resolution_rx = None;
    }

    /// Restarts the plugin host to apply a changed enabled-plugin set,
    /// running the heavy I/O in `bg_command_tasks` so the actor loop is never
    /// blocked.
    ///
    /// Shuts down the live host (stopping disabled plugins), spawns fresh DB
    /// IPC servers for the newly-detected plugin set, starts a new host with
    /// the updated configuration, removes only old factories that the new
    /// host does not replace, re-registers new plugin-provided LLM factories,
    /// re-bridges health events into diagnostics, and rebuilds the tool
    /// registry from the new host.
    ///
    /// The shared `plugin_host` and `health_bridge_handle` mutexes are updated
    /// directly by the background task. The actor-only fields (`self.registry`,
    /// `self.plugin_tool_registries`) are updated when the background task
    /// sends [`EneCommand::PluginHostReconfigured`] back through the mailbox.
    ///
    /// Remaining limitations:
    /// - The provider registry remains process-global, so concurrent handles
    ///   still use last-writer-wins registration for the same provider kind;
    ///   conditional deregistration avoids removing a replacement owned by a
    ///   different host.
    /// - The host-service acceptor spawned for the previous host is aborted
    ///   before the new one binds the shared socket path, so at most one live
    ///   listener serves plugins.
    fn spawn_reconfigure_plugin_host(&mut self) {
        if !admit_task(
            &mut self.bg_command_tasks,
            self.task_caps.bg_command_cap,
            "BgCommand",
            Some("PluginHostReconfigure".to_string()),
            &self.diag_tx,
        ) {
            tracing::warn!(
                component = "TurnActor",
                "Plugin host reconfiguration rejected: background task capacity exhausted"
            );
            return;
        }

        let config = self.config.clone();
        let memory_store = self.concrete_store.clone();
        let plugin_host = Arc::clone(&self.plugin_host);
        let health_bridge_handle = Arc::clone(&self.health_bridge_handle);
        let host_service_handle = Arc::clone(&self.host_service_handle);
        let diag_tx = self.diag_tx.clone();
        let cmd_tx = self.cmd_tx.clone();

        self.bg_command_tasks.spawn(async move {
            reconfigure_plugin_host_bg(
                config,
                memory_store,
                plugin_host,
                health_bridge_handle,
                host_service_handle,
                diag_tx,
                cmd_tx,
            )
            .await;
        });
    }

    /// Pushes updated plugin config/profiles blobs to live IPC connections.
    fn spawn_push_plugin_configs(
        &mut self,
        updates: HashMap<String, (Option<serde_json::Value>, Option<serde_json::Value>)>,
    ) {
        if !admit_task(
            &mut self.bg_command_tasks,
            self.task_caps.bg_command_cap,
            "BgCommand",
            Some("PluginConfigPush".to_string()),
            &self.diag_tx,
        ) {
            tracing::warn!(
                component = "TurnActor",
                "Plugin config push rejected: background task capacity exhausted"
            );
            return;
        }

        let plugin_host = Arc::clone(&self.plugin_host);
        self.bg_command_tasks.spawn(async move {
            let guard = plugin_host.lock().await;
            let Some(host) = guard.as_ref() else {
                return;
            };
            host.apply_plugin_configs(&updates).await;
        });
    }

    /// Kicks off proactive LLM initialization in the background if it has
    /// not already been started.
    ///
    /// Returns immediately in all cases — the actor loop is never blocked
    /// on a GGUF model load. Callers that need the handles should check
    /// `self.proactive_llm.get()` after calling this method.
    fn ensure_proactive_llm_non_blocking(&mut self) {
        if self.proactive_llm.get().is_some() {
            return;
        }
        if self.proactive_llm_init_spawned.swap(true, Ordering::AcqRel) {
            return;
        }

        if self.config.get_section::<ene_ai::AiConfig>().is_err() {
            // Reset so a later call (e.g. after a config hot-reload fixes
            // the section) can retry rather than being permanently stuck.
            self.proactive_llm_init_spawned
                .store(false, Ordering::Release);
            tracing::warn!(
                component = "Proactive",
                "Cannot init proactive LLM: config error"
            );
            return;
        }
        if !admit_task(
            &mut self.bg_command_tasks,
            self.task_caps.bg_command_cap,
            "BgCommand",
            Some("ProactiveLlmInit".to_string()),
            &self.diag_tx,
        ) {
            // No capacity right now; allow a later call to retry once a slot
            // frees up instead of leaving the flag latched forever.
            self.proactive_llm_init_spawned
                .store(false, Ordering::Release);
            return;
        }
        let cell = Arc::clone(&self.proactive_llm);
        let init_spawned = Arc::clone(&self.proactive_llm_init_spawned);
        let config = self.config.clone();
        let provider_host = Arc::clone(&self.provider_host);
        self.bg_command_tasks.spawn(async move {
            match crate::proactive_llm::build_proactive_llm_handles(&config, provider_host.as_ref())
                .await
            {
                Ok(handles) => {
                    tracing::info!(
                        component = "Proactive",
                        decision_backend = ?handles.decision_kind,
                        local_provider = handles.local().is_some(),
                        "Proactive decision provider ready (background)"
                    );
                    drop(cell.set(handles));
                }
                Err(e) => {
                    // A single transient failure must not permanently disable
                    // proactive features: reset the spawn guard so a later
                    // call retries. (On success the populated `OnceCell`
                    // short-circuits before this flag is ever consulted.)
                    init_spawned.store(false, Ordering::Release);
                    tracing::error!(
                        component = "Proactive",
                        error = %e,
                        "Failed to init proactive LLM in background"
                    );
                }
            }
        });
    }

    /// Handles [`EneCommand::PrepareVisionSummary`]: the busy-check
    /// and lazy local-model init for screen-image vision, minus the raw RGB
    /// buffer and the actual (expensive) model call — both of which now live
    /// entirely in [`crate::vision::VisionHandle`], outside this actor.
    ///
    /// Split into a fast path (handles already loaded → synchronous reply)
    /// and a slow path (handles still loading → background wait) so the
    /// actor loop is never blocked on a GGUF model load. The slow
    /// path is deduplicated: at most one background poller runs at a time
    /// (guarded by `vision_slow_path_active`), so a burst of concurrent
    /// requests during the loading window cannot exhaust `bg_command_cap`.
    fn prepare_vision_summary(
        &mut self,
        app_label: String,
        hints: crate::vision::ScreenSummaryHints,
        reply: oneshot::Sender<Result<VisionPrepared, crate::public_api::PublicApiError>>,
    ) {
        use crate::public_api::PublicApiError;

        if self.stream_handle.is_some() || self.proactive_decision_rx.is_some() {
            drop(reply.send(Err(PublicApiError::Internal {
                message: "runtime busy".to_string(),
            })));
            return;
        }

        let prompt_language = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .map_or_else(
                |_| "en".to_string(),
                |m| m.resolved_classifier_language().to_owned(),
            );

        self.ensure_proactive_llm_non_blocking();

        // Fast path: handles already loaded — complete synchronously.
        if let Some(handles) = self.proactive_llm.get() {
            let result = finish_vision_prep(handles, &prompt_language, &app_label, &hints);
            if result.is_ok() {
                // Mint a fresh cancel token for this request; a new user turn
                // cancels and replaces `self.vision_cancel` (see `EneCommand::Run`),
                // so an older, already-handed-out token being left cancelled from a
                // prior turn can never pre-cancel this new request.
                self.vision_cancel = CancellationToken::new();
                let cancel = self.vision_cancel.clone();
                drop(reply.send(result.map(|mut vp| {
                    vp.cancel = cancel;
                    vp
                })));
            } else {
                drop(reply.send(result));
            }
            return;
        }

        // Slow path: handles still loading — hand off to bg_command_tasks.
        // Deduplicate concurrent slow-path requests: the screen-capture
        // driven proactive flow can fire `PrepareVisionSummary` several times
        // during the GGUF loading window, and each call would otherwise spawn
        // a fresh up-to-five-minute poller. With `bg_command_cap` defaulting
        // to 4, that would exhaust the cap and starve unrelated background
        // work (e.g. a concurrent `PluginHostReconfigure`). Only one poller
        // runs at a time; concurrent callers fail fast and retry on the next
        // capture cycle once the model is loaded.
        if self.vision_slow_path_active.load(Ordering::Acquire) {
            drop(reply.send(Err(PublicApiError::Internal {
                message: "runtime busy: vision preparation already in progress".to_string(),
            })));
            return;
        }
        if !admit_task(
            &mut self.bg_command_tasks,
            self.task_caps.bg_command_cap,
            "BgCommand",
            Some("PrepareVisionSummary".to_string()),
            &self.diag_tx,
        ) {
            drop(reply.send(Err(PublicApiError::Internal {
                message: "runtime busy: background task capacity exhausted".to_string(),
            })));
            return;
        }

        // Mint the cancel token on the actor so `Run` can cancel it via
        // `self.vision_cancel` even though the reply is sent from a
        // background task.
        self.vision_cancel = CancellationToken::new();
        let cancel = self.vision_cancel.clone();
        let cell = Arc::clone(&self.proactive_llm);
        let slow_path_active = Arc::clone(&self.vision_slow_path_active);
        // Set only after admission succeeds, so a rejected admission does not
        // latch the flag and permanently block future slow-path requests.
        slow_path_active.store(true, Ordering::Release);
        self.bg_command_tasks.spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_mins(5);
            loop {
                if cancel.is_cancelled() {
                    slow_path_active.store(false, Ordering::Release);
                    drop(reply.send(Err(PublicApiError::Internal {
                        message: "vision preparation cancelled".to_string(),
                    })));
                    return;
                }
                if let Some(handles) = cell.get() {
                    let result = finish_vision_prep(handles, &prompt_language, &app_label, &hints)
                        .map(|mut vp| {
                            vp.cancel = cancel;
                            vp
                        });
                    slow_path_active.store(false, Ordering::Release);
                    drop(reply.send(result));
                    return;
                }
                if tokio::time::Instant::now() >= deadline {
                    slow_path_active.store(false, Ordering::Release);
                    drop(reply.send(Err(PublicApiError::Internal {
                        message: "proactive LLM init timed out".to_string(),
                    })));
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        });
    }

    async fn maybe_spawn_proactive_decision(&mut self) {
        if self.stream_handle.is_some() || self.proactive_decision_rx.is_some() {
            return;
        }
        if !self.pending_permissions.lock().await.is_empty()
            || !self.pending_user_inputs.lock().await.is_empty()
        {
            return;
        }
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        if !mind.proactive.enabled {
            return;
        }

        if mind.proactive.paused {
            if !self.proactive.quiet_hours_queue.is_empty() {
                tracing::info!(
                    component = "Proactive",
                    dropped = self.proactive.quiet_hours_queue.len(),
                    "Manual pause discards the pending quiet-hours catch-up queue"
                );
                self.proactive.quiet_hours_queue.clear();
            }
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "manual pause",
                "Proactive will not speak"
            );
            return;
        }

        let quiet = evaluate_quiet_hours(&mind.proactive.quiet_hours, (self.quiet_hours_clock)());
        if quiet.active && mind.proactive.quiet_hours.suppress.decisions {
            // Queue only real opportunities: the warrant gates (busy, idle,
            // cooldown, session limit, sources, fatigue) must have passed,
            // or a whole quiet night would saturate the queue with moments
            // the decision pipeline would have rejected anyway.
            if self.quiet_hours_opportunity(&mind, &quiet).await {
                self.on_quiet_hours_suppressed(&mind, &quiet);
            }
            return;
        }
        if !self.proactive.quiet_hours_queue.is_empty() {
            self.deliver_quiet_hours_catch_up(&mind).await;
            return;
        }

        self.ensure_proactive_llm_non_blocking();

        // If handles aren't ready yet, skip this tick — the next interval
        // will retry once the background load completes.
        let Some(handles) = self.proactive_llm.get() else {
            tracing::debug!(
                component = "Proactive",
                "Proactive decision deferred: LLM handles not yet loaded"
            );
            return;
        };

        tracing::info!(
            component = "Proactive",
            interval_seconds = mind.proactive.interval_seconds,
            min_idle_seconds = mind.proactive.min_idle_seconds,
            "Proactive decision started"
        );

        let decision_provider = Some(Arc::clone(&handles.decision));
        let epoch = self.proactive.epoch;
        let user_turn_busy = self.stream_handle.is_some()
            || !self.pending_permissions.lock().await.is_empty()
            || !self.pending_user_inputs.lock().await.is_empty();
        let suppression = self.proactive.suppression(user_turn_busy);
        let quiet_for_context =
            evaluate_quiet_hours(&mind.proactive.quiet_hours, (self.quiet_hours_clock)());
        let (history, observation, affect, commitments, user_instructions, pending_confirmation) =
            self.proactive_context_inputs(&mind).await;
        let world_state = self.proactive.world_state.clone();
        let (tx, rx) = oneshot::channel();
        self.proactive_decision_rx = Some(rx);
        let config = mind.proactive.clone();
        let prompt_language = mind.resolved_classifier_language().to_owned();
        let handle = tokio::spawn(async move {
            let result = crate::proactive::run_decision_task(
                config,
                history,
                observation,
                Some(world_state),
                suppression,
                quiet_for_context,
                pending_confirmation,
                decision_provider,
                epoch,
                affect,
                commitments,
                user_instructions,
                prompt_language,
            )
            .await;
            drop(tx.send(result));
        });
        self.proactive_decision_handle = Some(handle);
    }

    /// Load the live session inputs a proactive decision needs: history,
    /// observation, affect, active commitments, and standing rules. Shared
    /// by the decision spawn path and the quiet-hours opportunity check.
    async fn proactive_context_inputs(
        &self,
        mind: &ene_mind::MindConfig,
    ) -> (
        Vec<MindHistoryEntry>,
        ene_mind::ProactiveObservation,
        Option<ene_store::AffectState>,
        Vec<ene_mind::ActiveCommitmentPrompt>,
        Vec<String>,
        Option<ene_mind::PendingConfirmationPrompt>,
    ) {
        let observation = self.proactive.observation.clone();
        let history = self.session.history().to_vec();
        let card_name = self.session.card_name().to_string();
        let user_name = self.config.user_name.clone();
        let mem_store = self.session.memory.memory_store.clone();
        let (affect, commitments, user_instructions, pending_confirmation) =
            if let Some(store) = mem_store.as_ref() {
                let affect = store.get_affect_state(&card_name).await.ok();
                let raw = store
                    .list_active_commitments(&card_name, Some(user_name.as_str()), 10)
                    .await
                    .unwrap_or_default();
                let commitments = CommitmentLedger::active_prompt_candidates(&raw);
                // Deterministic suppression-condition injection: loaded without
                // recall scoring so a "don't talk" instruction can never lose a
                // score competition. Errors degrade to no notes (fail-closed
                // silence is the decision model's job, not the loader's).
                let user_instructions = if mind.proactive.sources.memory {
                    ene_mind::load_proactive_memory_notes(
                        store.as_ref(),
                        &card_name,
                        &user_name,
                        mind.proactive.max_memory_notes,
                    )
                    .await
                    .unwrap_or_default()
                } else {
                    Vec::new()
                };
                // Only one confirmation question may be in flight; a due
                // candidate is skipped while a delivered question still awaits
                // its reply.
                let pending_confirmation = if self.proactive.asked_pending_candidate.is_none() {
                    ene_mind::load_due_pending_confirmation(
                        self.session.memory.recall_cache.as_deref(),
                        store.as_ref(),
                        &card_name,
                        &user_name,
                        &mind.proactive.pending_confirmation,
                        (self.quiet_hours_clock)(),
                        &self.proactive.pending_confirmation_asked_at,
                    )
                    .await
                } else {
                    None
                };
                (affect, commitments, user_instructions, pending_confirmation)
            } else {
                (None, Vec::new(), Vec::new(), None)
            };
        (
            history,
            observation,
            affect,
            commitments,
            user_instructions,
            pending_confirmation,
        )
    }

    /// Whether a quiet-hours-suppressed tick was a real utterance
    /// opportunity: every deterministic warrant gate (user busy, idle,
    /// cooldown, session limit, sources, fatigue) would have passed. Quiet
    /// hours are evaluated last by the gate, so a `QuietHours` rejection is
    /// exactly that signal.
    async fn quiet_hours_opportunity(
        &self,
        mind: &ene_mind::MindConfig,
        quiet: &ene_mind::QuietHoursEval,
    ) -> bool {
        let user_turn_busy = self.stream_handle.is_some()
            || !self.pending_permissions.lock().await.is_empty()
            || !self.pending_user_inputs.lock().await.is_empty();
        let suppression = self.proactive.suppression(user_turn_busy);
        let (history, observation, affect, commitments, user_instructions, pending_confirmation) =
            self.proactive_context_inputs(mind).await;
        let context = build_proactive_context(
            &mind.proactive,
            &history,
            &observation,
            affect.as_ref(),
            &commitments,
            &user_instructions,
            suppression,
            quiet.clone(),
            pending_confirmation,
            Some(&self.proactive.world_state),
        );
        matches!(
            evaluate_deterministic_gates(&mind.proactive, &context),
            Err(GateRejectReason::QuietHours)
        )
    }

    async fn handle_proactive_decision(
        &mut self,
        result: crate::proactive::ProactiveDecisionResult,
    ) {
        if result.epoch != self.proactive.epoch {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "stale after user turn",
                "Proactive will not speak"
            );
            return;
        }
        if self.stream_handle.is_some() {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "user stream active",
                "Proactive will not speak"
            );
            return;
        }
        if !result.should_generate {
            tracing::info!(
                component = "Proactive",
                speak = false,
                should_speak = result.should_speak,
                confidence = result.confidence,
                llm_invoked = result.llm_invoked,
                detail = %result.detail,
                "Proactive will not speak"
            );
            return;
        }

        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let language = mind.resolved_classifier_language();
        let hint = if let Some(candidate) = &result.pending_confirmation {
            crate::proactive::proactive_pending_confirmation_hint(
                candidate,
                language,
                mind.proactive.confirmation_enabled,
            )
        } else {
            crate::proactive::proactive_generation_hint(
                &result.topic_hint,
                language,
                mind.proactive.confirmation_enabled,
            )
        };
        self.begin_proactive_generation(&result, hint).await;
    }

    /// Start the proactive generation stream for a decided (or catch-up)
    /// utterance. Owns the turn gate, screen-image stash, status
    /// announcement, and stream spawn shared by decision and catch-up paths.
    /// Returns false when the gate is busy, suppression re-engaged between
    /// decision and generation, or the stream could not start (caller keeps
    /// its state, e.g. queued catch-up moments).
    async fn begin_proactive_generation(
        &mut self,
        result: &crate::proactive::ProactiveDecisionResult,
        hint: String,
    ) -> bool {
        // Re-check the deterministic suppression state at generation start: a
        // decision that passed the gate earlier must not begin speaking once
        // the user paused or the quiet-hours window opened in the meantime.
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let quiet = evaluate_quiet_hours(&mind.proactive.quiet_hours, (self.quiet_hours_clock)());
        if mind.proactive.paused {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "manual pause",
                "Proactive will not speak"
            );
            return false;
        }
        if quiet.active && mind.proactive.quiet_hours.suppress.decisions {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "quiet hours",
                "Proactive will not speak"
            );
            return false;
        }

        let turn = TurnId::new();
        if !self.turn_gate.try_begin(&turn) {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "turn gate busy",
                "Proactive will not speak"
            );
            return false;
        }
        tracing::info!(
            component = "Proactive",
            speak = true,
            confidence = result.confidence,
            topic_hint = %result.topic_hint,
            detail = %result.detail,
            confirmation = %result.confirmation,
            "Proactive will speak"
        );
        self.proactive.last_decision = Some(result.clone());
        let screen_image = if result.catch_up {
            // A catch-up utterance only knows that moments occurred, never
            // their content; a stashed frame would contradict the note.
            let _ = self.proactive.take_screen_image();
            None
        } else {
            self.config
                .get_section::<ene_ai::AiConfig>()
                .ok()
                .filter(ene_ai::AiConfig::proactive_generation_supports_vision)
                .and_then(|_| self.proactive.take_screen_image())
        };
        if screen_image.is_none() {
            // Drop any stashed frame when the generation model cannot use it.
            let _ = self.proactive.take_screen_image();
        }
        self.drain_pending().await;
        self.cancel_token = CancellationToken::new();
        self.terminal_emitted = Arc::new(AtomicBool::new(false));
        self.active_turn = Some(turn.clone());
        self.active_origin = crate::types::TurnOrigin::Proactive;
        self.quiet_hours_notifications_suppressed =
            crate::proactive::quiet_hours_suppresses_notifications(
                &quiet,
                mind.proactive.quiet_hours.suppress,
            );
        if self.quiet_hours_notifications_suppressed {
            tracing::info!(
                component = "Proactive",
                event = "quiet_hours_notification_suppressed",
                weekday = %quiet.weekday,
                local_time = %quiet.local_time,
                "Proactive status announcement suppressed by quiet hours"
            );
        } else {
            drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                status: EneStatus::Running,
            }));
        }
        let generation_timeout =
            std::time::Duration::from_secs(mind.proactive.generation_timeout_seconds.max(1));
        self.start_stream(
            String::new(),
            turn,
            crate::types::TurnOrigin::Proactive,
            false,
            mind.proactive.allow_tools,
            Some(hint),
            screen_image,
            Some(result.topic_hint.clone()),
            Some(generation_timeout),
        )
        .await
    }

    /// Classify the just-completed user turn as the reply to an outstanding
    /// confirmation question and spawn the resolution task. Fail-closed:
    /// when no provider, turn id, or reply text is available, the marker is
    /// restored so the next user turn retries instead of dropping the reply
    /// classification entirely.
    fn spawn_pending_resolution(&mut self) {
        if self.proactive_resolution_rx.is_some() {
            return;
        }
        let Some(candidate) = self.proactive.asked_pending_candidate.take() else {
            return;
        };
        let Some(handles) = self.proactive_llm.get() else {
            tracing::warn!(
                component = "Proactive",
                event = "pending_resolution",
                candidate_id = candidate.id,
                "Reply classification deferred: proactive LLM handles not ready"
            );
            self.proactive.asked_pending_candidate = Some(candidate);
            return;
        };
        let Some(turn) = self.active_turn.clone() else {
            self.proactive.asked_pending_candidate = Some(candidate);
            return;
        };
        let reply = self
            .session
            .history()
            .iter()
            .rev()
            .find(|entry| entry.role == ene_ai::Role::User)
            .map_or_else(String::new, |entry| entry.content.clone());
        if reply.trim().is_empty() {
            self.proactive.asked_pending_candidate = Some(candidate);
            return;
        }

        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let session_epoch = self.proactive.session_epoch;
        let provider = Arc::clone(&handles.decision);
        let prompt_language = mind.resolved_classifier_language().to_owned();
        let timeout =
            std::time::Duration::from_secs(mind.proactive.decision_timeout_seconds.max(1));
        let (tx, rx) = oneshot::channel();
        self.proactive_resolution_rx = Some(rx);
        let handle = tokio::spawn(async move {
            let result = crate::proactive::run_resolution_task(
                candidate,
                reply,
                turn,
                session_epoch,
                provider,
                prompt_language,
                timeout,
            )
            .await;
            drop(tx.send(result));
        });
        self.proactive_resolution_handle = Some(handle);
    }

    /// Apply a reply-classification verdict: approve or reject the candidate
    /// through the store, invalidate the recall cache, and emit
    /// `CandidateChanged`. `Unclear` and stale (post-reset) results leave the
    /// candidate pending; claim-based store mutations make a double
    /// resolution harmless.
    async fn handle_proactive_resolution(
        &mut self,
        result: crate::proactive::PendingResolutionResult,
    ) {
        if result.session_epoch != self.proactive.session_epoch {
            tracing::info!(
                component = "Proactive",
                event = "pending_resolution",
                candidate_id = result.candidate_id,
                "Stale pending-candidate resolution ignored after session reset"
            );
            return;
        }
        let status = match result.verdict {
            ene_mind::PendingResolutionVerdict::Approved => {
                ene_store::PendingCandidateStatus::Approved
            }
            ene_mind::PendingResolutionVerdict::Rejected => {
                ene_store::PendingCandidateStatus::Rejected
            }
            ene_mind::PendingResolutionVerdict::Unclear => {
                tracing::info!(
                    component = "Proactive",
                    event = "pending_resolution",
                    candidate_id = result.candidate_id,
                    detail = %result.detail,
                    "Pending candidate reply was unclear; leaving it pending"
                );
                return;
            }
        };
        let Some(store) = self.concrete_store.clone() else {
            tracing::warn!(
                component = "Proactive",
                event = "pending_resolution",
                candidate_id = result.candidate_id,
                "Memory store unavailable; candidate left pending"
            );
            return;
        };
        let outcome = match status {
            ene_store::PendingCandidateStatus::Approved => {
                let approved = store.approve_pending_candidate(result.candidate_id).await;
                if let Ok(memory_id) = approved
                    && let Ok(mind_cfg) = self.config.get_section::<ene_mind::MindConfig>()
                    && mind_cfg.memory.reflection.enabled
                    && let Ok(Some(candidate)) =
                        store.get_pending_candidate(result.candidate_id).await
                {
                    ene_mind::memory_writer::reflection::record_approved_outcome(
                        store.as_ref(),
                        &candidate,
                        memory_id,
                    )
                    .await;
                }
                approved.map(|_| ())
            }
            ene_store::PendingCandidateStatus::Rejected => {
                store
                    .resolve_pending_candidate(result.candidate_id, false)
                    .await
            }
            ene_store::PendingCandidateStatus::Pending => return,
        };
        match outcome {
            Ok(()) => {
                self.proactive
                    .pending_confirmation_asked_at
                    .remove(&result.candidate_id);
                if let Some(cache) = &self.session.memory.recall_cache {
                    cache.invalidate_character(self.session.card_name());
                }
                drop(self.lifecycle_tx.send(LifecycleEvent::CandidateChanged {
                    id: result.candidate_id,
                    status,
                    turn: Some(result.turn),
                }));
                tracing::info!(
                    component = "Proactive",
                    event = "pending_resolution",
                    candidate_id = result.candidate_id,
                    status = ?status,
                    "Pending candidate resolved by user reply"
                );
            }
            Err(error) => {
                tracing::warn!(
                    component = "Proactive",
                    event = "pending_resolution",
                    candidate_id = result.candidate_id,
                    status = ?status,
                    error = %error,
                    "Pending candidate resolution failed; leaving it pending"
                );
            }
        }
    }

    /// Record one quiet-hours-suppressed moment: bounded queue entry for the
    /// `queue` / `summary` policies plus the structured suppression log.
    /// The log carries decision + policy metadata only — never screen data.
    fn on_quiet_hours_suppressed(
        &mut self,
        mind: &ene_mind::MindConfig,
        quiet: &ene_mind::QuietHoursEval,
    ) {
        let policy = mind.proactive.quiet_hours.policy;
        if policy != QuietHoursPolicy::Discard {
            let queue = &mut self.proactive.quiet_hours_queue;
            if queue.len() >= crate::proactive::QUIET_HOURS_QUEUE_CAP {
                queue.pop_front();
                tracing::debug!(
                    component = "Proactive",
                    "Quiet-hours queue full; dropping the oldest moment"
                );
            }
            queue.push_back(crate::proactive::QueuedQuietHour {
                local_date: quiet.local_date.clone(),
                local_time: quiet.local_time.clone(),
            });
        }
        let suppress = &mind.proactive.quiet_hours.suppress;
        tracing::info!(
            component = "Proactive",
            event = "quiet_hours_suppression",
            policy = %policy,
            suppress_notifications = suppress.notifications,
            suppress_decisions = suppress.decisions,
            suppress_tts = suppress.tts,
            weekday = %quiet.weekday,
            local_time = %quiet.local_time,
            timezone = %quiet.timezone,
            queue_len = self.proactive.quiet_hours_queue.len(),
            "Proactive decision suppressed by quiet hours"
        );
    }

    /// Deliver queued quiet-hours moments as catch-up utterances.
    ///
    /// `queue` policy: one turn per moment, paced one per tick. `summary`
    /// policy: one aggregated turn. `discard` policy (or a policy change to
    /// it) drops the queue. Called once per tick while the queue is non-empty
    /// and quiet hours are inactive, so a busy stream paces delivery.
    async fn deliver_quiet_hours_catch_up(&mut self, mind: &ene_mind::MindConfig) {
        if self.proactive.quiet_hours_queue.is_empty() {
            return;
        }
        if mind.proactive.paused {
            self.proactive.quiet_hours_queue.clear();
            return;
        }
        let policy = mind.proactive.quiet_hours.policy;
        let language = mind.resolved_classifier_language().to_owned();
        match policy {
            QuietHoursPolicy::Discard => {
                self.proactive.quiet_hours_queue.clear();
            }
            QuietHoursPolicy::Queue => {
                let Some(entry) = self.proactive.quiet_hours_queue.front().cloned() else {
                    return;
                };
                let items = crate::proactive::quiet_hours_items(std::slice::from_ref(&entry));
                let hint = crate::proactive::quiet_hours_catch_up_hint(&items, &language);
                tracing::info!(
                    component = "Proactive",
                    event = "quiet_hours_catch_up",
                    policy = %policy,
                    items = %items,
                    "Quiet hours ended; delivering catch-up speech"
                );
                let result = self.synthetic_catch_up_result("quiet hours catch-up");
                let started = self.begin_proactive_generation(&result, hint).await;
                if started {
                    self.proactive.quiet_hours_queue.pop_front();
                }
            }
            QuietHoursPolicy::Summary => {
                let keep = self
                    .proactive
                    .quiet_hours_queue
                    .len()
                    .min(crate::proactive::QUIET_HOURS_SUMMARY_MAX_ITEMS);
                let dropped = self.proactive.quiet_hours_queue.len().saturating_sub(keep);
                let entries: Vec<_> = self
                    .proactive
                    .quiet_hours_queue
                    .iter()
                    .take(keep)
                    .cloned()
                    .collect();
                let items = crate::proactive::quiet_hours_items(&entries);
                let hint = crate::proactive::quiet_hours_catch_up_hint(&items, &language);
                tracing::info!(
                    component = "Proactive",
                    event = "quiet_hours_catch_up",
                    policy = %policy,
                    items = %items,
                    dropped = dropped,
                    "Quiet hours ended; delivering catch-up summary"
                );
                let result = self.synthetic_catch_up_result("quiet hours summary");
                let started = self.begin_proactive_generation(&result, hint).await;
                if started {
                    self.proactive.quiet_hours_queue.clear();
                }
            }
        }
    }

    /// Synthetic decision result for a quiet-hours catch-up generation: no
    /// decision LLM ran, so confidence is pinned and the confirmation stage
    /// stays disabled (the catch-up note carries the refusal freedom).
    fn synthetic_catch_up_result(
        &self,
        topic_hint: &str,
    ) -> crate::proactive::ProactiveDecisionResult {
        crate::proactive::ProactiveDecisionResult {
            epoch: self.proactive.epoch,
            should_generate: true,
            should_speak: true,
            confidence: 1.0,
            llm_invoked: false,
            topic_hint: topic_hint.to_string(),
            detail: "quiet hours ended".to_string(),
            confirmation: ene_mind::ProactiveConfirmation::Disabled,
            catch_up: true,
            pending_confirmation: None,
        }
    }

    /// TTS provider for a proactive turn: dropped while quiet hours suppress
    /// speech audio, so the stream displays text without speaking it.
    fn proactive_tts_provider(
        &self,
        quiet: &ene_mind::QuietHoursEval,
        suppress: ene_mind::QuietHoursSuppressConfig,
    ) -> Option<Arc<dyn ene_ai::TtsProvider>> {
        if crate::proactive::quiet_hours_suppresses_tts(quiet, suppress) {
            None
        } else {
            self.tts_provider.clone()
        }
    }

    /// Rebuilds the TTS provider after a plugin-host restart.
    ///
    /// The previous provider instance (built at bootstrap) holds an IPC
    /// connection to the old host's plugin process, which `shutdown()` has
    /// killed, so it must be replaced by a fresh instance from the live host
    /// registry. Mirrors the bootstrap path's failure handling: an unbuildable
    /// provider disables TTS with a warning rather than failing the turn.
    async fn rebuild_tts_provider(&mut self) {
        let ai_config = match self.config.get_section::<ene_ai::AiConfig>() {
            Ok(config) => config,
            Err(e) => {
                self.tts_provider = None;
                tracing::warn!(
                    component = "TurnActor",
                    error = %e,
                    "Cannot rebuild TTS provider after plugin reconfiguration: AI config error"
                );
                return;
            }
        };
        let Some(resolved) = ai_config.resolve_tts() else {
            self.tts_provider = None;
            return;
        };
        match self
            .provider_host
            .create_tts_provider(&resolved.provider, &self.config)
            .await
        {
            Ok(provider) => {
                self.tts_provider = Some(Arc::from(provider));
                tracing::info!(
                    component = "TurnActor",
                    provider = %resolved.provider,
                    "Rebuilt TTS provider after plugin reconfiguration"
                );
            }
            Err(e) => {
                self.tts_provider = None;
                tracing::warn!(
                    component = "TurnActor",
                    provider = %resolved.provider,
                    error = %e,
                    "Failed to rebuild TTS provider after plugin reconfiguration; \
                     audio synthesis disabled"
                );
            }
        }
    }

    /// Reconcile scheduler state left by a crash and arm startup schedules
    /// for this process start. Runs before the command loop so the timer
    /// task (spawned afterwards) sees a consistent store.
    async fn reconcile_scheduler_startup(&mut self) {
        let Some(store) = self.concrete_store.clone() else {
            return;
        };
        let enabled = self
            .config
            .get_section::<crate::scheduler::SchedulerConfig>()
            .map_or(true, |cfg| cfg.enabled);
        if !enabled {
            return;
        }
        let now = (self.scheduler_clock)();
        if let Err(e) = store.reconcile_startup(now).await {
            tracing::error!(
                component = "Scheduler",
                error = %e,
                "Failed to reconcile schedule runs at startup"
            );
        }
        if let Err(e) = store.arm_startup_schedules(now).await {
            tracing::error!(
                component = "Scheduler",
                error = %e,
                "Failed to arm startup schedules"
            );
        }
    }

    /// Spawn the scheduler timer task when the store and config allow it.
    fn spawn_scheduler_task(&mut self) {
        let Some(store) = self.concrete_store.clone() else {
            return;
        };
        let enabled = self
            .config
            .get_section::<crate::scheduler::SchedulerConfig>()
            .map_or(true, |cfg| cfg.enabled);
        let Some(notify_rx) = self.scheduler_notify_rx.take() else {
            return;
        };
        if !enabled {
            return;
        }
        let cmd_tx = self.cmd_tx.clone();
        let scheduler_clock = Arc::clone(&self.scheduler_clock);
        self.aux_tasks.spawn(async move {
            crate::scheduler::task::run(store, cmd_tx, notify_rx, scheduler_clock).await;
        });
    }

    /// Wake the scheduler timer task so it re-derives due times.
    fn notify_scheduler(&self) {
        // The error value is `Copy` (it wraps `()`), so `drop()` would trip
        // `clippy::dropping_copy_types`.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "watch send error is Copy; drop() would trip dropping_copy_types"
        )]
        let _ = self.scheduler_notify.send(());
    }

    /// Process one due schedule fire: apply the late-execution and busy
    /// policies, claim the occurrence atomically, then execute or wait.
    async fn handle_schedule_fire(
        &mut self,
        schedule_id: i64,
        scheduled_at: chrono::DateTime<chrono::Utc>,
    ) {
        let Some(store) = self.concrete_store.clone() else {
            self.notify_scheduler();
            return;
        };
        let cfg = self
            .config
            .get_section::<crate::scheduler::SchedulerConfig>()
            .unwrap_or_default();
        let now = (self.scheduler_clock)();
        let Ok(Some(schedule)) = store.get_schedule(schedule_id).await else {
            // Notify even on read failure: the timer's `queued` set must not
            // keep suppressing a due fire after a transient DB error.
            self.notify_scheduler();
            return; // deleted, or the store failed; nothing to execute
        };
        if !schedule.enabled || schedule.next_run_at.as_ref() != Some(&scheduled_at) {
            self.notify_scheduler();
            return; // paused or a stale duplicate dispatch
        }

        let grace =
            chrono::Duration::from_std(std::time::Duration::from_secs(cfg.late_grace_secs.max(1)))
                .unwrap_or_default();
        let late = now.signed_duration_since(scheduled_at) > grace;
        let turn = TurnId::new();
        let gate_held = self.turn_gate.try_begin(&turn);
        let mode = if late {
            ene_store::FireClaimMode::SkipLate
        } else if !gate_held {
            ene_store::FireClaimMode::SkipBusy
        } else if schedule.confirmation == ScheduleConfirmation::Confirm {
            ene_store::FireClaimMode::AwaitConfirmation
        } else {
            ene_store::FireClaimMode::Execute
        };

        let claimed = match store.claim_fire(schedule_id, scheduled_at, now, mode).await {
            Ok(Some(claimed)) => claimed,
            Ok(None) => {
                if gate_held {
                    self.turn_gate.end();
                }
                self.notify_scheduler();
                return;
            }
            Err(e) => {
                if gate_held {
                    self.turn_gate.end();
                }
                tracing::error!(
                    component = "Scheduler",
                    error = %e,
                    schedule_id,
                    "Failed to claim schedule fire"
                );
                self.notify_scheduler();
                return;
            }
        };

        match mode {
            ene_store::FireClaimMode::Execute => {
                self.begin_scheduled_run(claimed, turn).await;
            }
            ene_store::FireClaimMode::AwaitConfirmation => {
                // Never hold the single-flight gate while waiting for the
                // user: a conversation can start during the confirmation.
                self.turn_gate.end();
                self.register_schedule_confirmation(claimed, cfg.confirmation_timeout_secs);
            }
            ene_store::FireClaimMode::SkipBusy | ene_store::FireClaimMode::SkipLate => {}
        }
        self.notify_scheduler();
    }

    /// Emit the confirmation prompt for a claimed run and arm its timeout.
    fn register_schedule_confirmation(
        &mut self,
        claimed: ene_store::ClaimedFire,
        timeout_secs: u64,
    ) {
        let ene_store::ClaimedFire {
            schedule, run_id, ..
        } = claimed;
        let schedule_id = schedule.id;
        let schedule_name = schedule.name.clone();
        let request_id = RequestId::new(format!("schedule-{run_id}"));
        let timeout_token = CancellationToken::new();
        self.pending_schedule_confirmations.insert(
            request_id.clone(),
            PendingScheduleConfirmation {
                schedule_id,
                run_id,
                timeout: timeout_token.clone(),
            },
        );
        let description = match &schedule.action {
            ene_core::ScheduleAction::Tool { name, .. } => {
                format!("Schedule `{schedule_name}` wants to run the tool `{name}`")
            }
            ene_core::ScheduleAction::Prompt { .. } => {
                format!("Schedule `{schedule_name}` wants to run a scheduled action")
            }
        };
        drop(self.event_tx.send(EneEvent::PermissionRequired {
            turn: TurnId::new(),
            origin: crate::types::TurnOrigin::Scheduled,
            request_id: request_id.clone(),
            action: "schedule.run".to_string(),
            target: schedule_name,
            description,
        }));

        let cmd_tx = self.cmd_tx.clone();
        let timeout = std::time::Duration::from_secs(timeout_secs.max(1));
        self.aux_tasks.spawn(async move {
            tokio::select! {
                () = timeout_token.cancelled() => {}
                () = tokio::time::sleep(timeout) => {
                    drop(cmd_tx.send(EneCommand::ScheduleConfirmationTimeout {
                        request_id,
                        schedule_id,
                        run_id,
                    }));
                }
            }
        });
    }

    /// Start a confirmed run. The claim already happened (the run row is
    /// `awaiting_approval`), so this only acquires the gate and executes.
    async fn begin_approved_scheduled_run(&mut self, pending: PendingScheduleConfirmation) {
        let Some(store) = self.concrete_store.clone() else {
            return;
        };
        let Ok(Some(schedule)) = store.get_schedule(pending.schedule_id).await else {
            return; // deleted while waiting; the run row stays open until the
            // next startup reconciliation
        };
        if !schedule.enabled {
            self.finish_scheduled_run(
                pending.schedule_id,
                pending.run_id,
                ScheduleRunStatus::Failed,
                Some("schedule disabled while awaiting confirmation".to_string()),
            )
            .await;
            return;
        }
        let turn = TurnId::new();
        if !self.turn_gate.try_begin(&turn) {
            // A conversation started while the confirmation was pending.
            self.finish_scheduled_run(
                pending.schedule_id,
                pending.run_id,
                ScheduleRunStatus::SkippedBusy,
                None,
            )
            .await;
            return;
        }
        // Move the row out of `awaiting_approval` before executing so a
        // stale timeout can never record `timed_out` for a run that is
        // actually running (and a crash reconciles as `interrupted`).
        let now = (self.scheduler_clock)();
        if let Err(e) = store
            .mark_run_running(pending.schedule_id, pending.run_id, now)
            .await
        {
            self.turn_gate.end();
            tracing::error!(
                component = "Scheduler",
                error = %e,
                schedule_id = pending.schedule_id,
                run_id = pending.run_id,
                "Failed to mark approved schedule run as running"
            );
            return;
        }
        self.begin_scheduled_run(
            ene_store::ClaimedFire {
                schedule,
                run_id: pending.run_id,
                is_retry: false,
            },
            turn,
        )
        .await;
        self.notify_scheduler();
    }

    /// Execute a claimed schedule action under the held single-flight gate.
    async fn begin_scheduled_run(&mut self, claimed: ene_store::ClaimedFire, turn: TurnId) {
        self.active_turn = Some(turn.clone());
        self.active_origin = crate::types::TurnOrigin::Scheduled;
        self.cancel_token = CancellationToken::new();
        self.terminal_emitted = Arc::new(AtomicBool::new(false));
        self.active_scheduled_run = Some(ActiveScheduledRun {
            schedule_id: claimed.schedule.id,
            run_id: claimed.run_id,
        });
        drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
            status: EneStatus::Running,
        }));
        if let ene_core::ScheduleAction::Tool { name, arguments } = &claimed.schedule.action {
            let name = name.clone();
            let arguments_json = arguments.to_string();
            drop(self.event_tx.send(EneEvent::TurnStarted {
                turn: turn.clone(),
                origin: crate::types::TurnOrigin::Scheduled,
            }));
            self.spawn_scheduled_tool_task(claimed, turn, name, arguments_json);
        } else if let ene_core::ScheduleAction::Prompt { text, allow_tools } =
            &claimed.schedule.action
        {
            let text = text.clone();
            let allow_tools = *allow_tools;
            self.start_stream(
                text,
                turn,
                crate::types::TurnOrigin::Scheduled,
                false,
                allow_tools,
                None,
                None,
                None,
                None,
            )
            .await;
            // `start_stream` fails synchronously (emitting Terminal and
            // releasing the gate) when the provider cannot open; every
            // other terminal path is observed via `stream_session_rx`.
            if self.terminal_emitted.load(Ordering::Acquire) {
                self.active_scheduled_run = None;
                self.finish_scheduled_run(
                    claimed.schedule.id,
                    claimed.run_id,
                    ScheduleRunStatus::Failed,
                    Some("scheduled prompt could not start".to_string()),
                )
                .await;
            }
            drop(claimed);
        }
    }

    /// Spawn the execution of a scheduled tool action as a supervised task.
    fn spawn_scheduled_tool_task(
        &mut self,
        claimed: ene_store::ClaimedFire,
        turn: TurnId,
        name: String,
        arguments_json: String,
    ) {
        let ene_store::ClaimedFire {
            schedule, run_id, ..
        } = claimed;
        let schedule_id = schedule.id;
        let final_turn = turn.clone();
        let final_name = name.clone();
        if !admit_task(
            &mut self.call_tool_tasks,
            self.task_caps.call_tool_cap,
            "ScheduledTool",
            Some(name.clone()),
            &self.diag_tx,
        ) {
            // The task set is at capacity: fail the run rather than queueing
            // it (matching the direct `CallTool` admission contract). The
            // failure arms a retry when the schedule allows one.
            let cmd_tx = self.cmd_tx.clone();
            self.aux_tasks.spawn(async move {
                drop(cmd_tx.send(EneCommand::ScheduleToolFinished {
                    schedule_id,
                    run_id,
                    turn: final_turn,
                    tool_name: final_name,
                    denied: false,
                    result: Err("scheduled tool execution rejected: task queue full".to_string()),
                }));
            });
            return;
        }
        let registry = self.registry.clone();
        let tool_rag = self.tool_rag.clone();
        let event_tx = self.event_tx.clone();
        let pending_permissions = self.pending_permissions.clone();
        let pending_user_inputs = self.pending_user_inputs.clone();
        let permission_scopes = self.permission_scopes.clone();
        let cancel_token = self.cancel_token.clone();
        let cmd_tx = self.cmd_tx.clone();
        let concrete_store = self.concrete_store.clone();
        let session_id = self.session.memory.session_id.to_string();
        let card_name = self.session.card_name().to_string();
        let plugin_cfg = self
            .config
            .get_section::<ene_plugin_host::PluginConfig>()
            .unwrap_or_default();
        let tool_timeout = std::time::Duration::from_millis(plugin_cfg.timeout_ms);
        let args_json = arguments_json;
        self.call_tool_tasks.spawn(async move {
            // The panic guard mirrors the stream task: a panicked tool task
            // must still report back so the actor releases the gate and
            // records a terminal run instead of leaking both until restart.
            let (outcome, denied) = if let Ok(result) =
                futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async move {
                    let call_ctx = ene_plugin_proto::CallContext {
                        conversation_id: session_id.clone(),
                        turn_id: turn.to_string(),
                    };
                    let dispatch = if cancel_token.is_cancelled() {
                        Err(ene_plugin_host::PluginHostError::ExecutionFailed {
                            message: "scheduled tool run cancelled before dispatch".to_string(),
                        })
                    } else if name == "system.search_tools" {
                        let query = serde_json::from_str::<serde_json::Value>(&args_json)
                            .ok()
                            .and_then(|v| v.get("query").and_then(|q| q.as_str()).map(String::from))
                            .unwrap_or_default();
                        crate::streaming::execute_system_search_tool(
                            registry.as_ref(),
                            tool_rag.as_deref(),
                            &query,
                            &card_name,
                        )
                        .await
                    } else {
                        tokio::time::timeout(
                            tool_timeout,
                            registry.call_tool(&name, &args_json, Some(&call_ctx)),
                        )
                        .await
                        .map_or_else(
                            |_| Err(crate::streaming::timeout_error(&name, tool_timeout)),
                            |r| r.map(|r| r.text_for_llm()),
                        )
                    };
                    drop(event_tx.send(EneEvent::ToolCallStart {
                        turn: turn.clone(),
                        origin: crate::types::TurnOrigin::Scheduled,
                        name: name.clone(),
                        arguments: args_json.clone(),
                    }));
                    let resolved = crate::streaming::resolve_tool_prompts(
                        &crate::streaming::ToolPromptResolution {
                            event_tx: &event_tx,
                            turn: &turn,
                            origin: crate::types::TurnOrigin::Scheduled,
                            pending_permissions: &pending_permissions,
                            pending_user_inputs: &pending_user_inputs,
                            permission_scopes: &permission_scopes,
                            cancel_token: &cancel_token,
                            permission_prompt_timeout_ms: plugin_cfg.permission_prompt_timeout_ms,
                            user_input_prompt_timeout_ms: plugin_cfg.user_input_prompt_timeout_ms,
                        },
                        registry.as_ref(),
                        &name,
                        &args_json,
                        &call_ctx,
                        tool_timeout,
                        dispatch,
                    )
                    .await;
                    if let Some(store) = concrete_store {
                        let success = resolved.result.is_ok();
                        ene_store::MemoryStore::spawn_insert_audit_entry(
                            &store,
                            ene_store::NewAuditEntry {
                                turn_id: turn.to_string(),
                                session_id: Some(session_id),
                                tool_name: name.clone(),
                                action: resolved.audit_action,
                                target: resolved.audit_target,
                                decision: resolved.audit_decision,
                                success,
                                arguments: args_json.clone(),
                            },
                        );
                    }
                    let outcome = match resolved.result {
                        Ok(text) => Ok(text),
                        Err(e) => Err(e.to_string()),
                    };
                    (outcome, resolved.denied)
                }))
                .await
            {
                result
            } else {
                tracing::error!(
                    component = "ScheduledTool",
                    tool = %final_name,
                    "Scheduled tool task panicked"
                );
                (Err("scheduled tool task panicked".to_string()), false)
            };
            drop(cmd_tx.send(EneCommand::ScheduleToolFinished {
                schedule_id,
                run_id,
                turn: final_turn,
                tool_name: final_name,
                denied,
                result: outcome,
            }));
        });
    }

    /// Record a terminal run outcome and wake the timer task.
    async fn finish_scheduled_run(
        &mut self,
        schedule_id: i64,
        run_id: i64,
        status: ScheduleRunStatus,
        error: Option<String>,
    ) {
        let Some(store) = self.concrete_store.clone() else {
            return;
        };
        if let Err(e) = store
            .finish_run(schedule_id, run_id, status, error, (self.scheduler_clock)())
            .await
        {
            tracing::error!(
                component = "Scheduler",
                error = %e,
                schedule_id,
                run_id,
                "Failed to record schedule run outcome"
            );
        }
        self.notify_scheduler();
    }

    /// `pub(super)` (rather than private) solely so `handle::tests` can drive
    /// individual commands directly — e.g. to test Stage 8 task-admission
    /// caps deterministically without racing the run loop's reap timing
    /// against a mock tool's completion speed.
    pub(super) async fn handle_command(&mut self, cmd: EneCommand) -> bool {
        match cmd {
            EneCommand::Run { input, turn } => {
                // Single-flight: Busy is enforced on the handle via TurnGate.
                // Never abort an in-flight turn here.
                if self.stream_handle.is_some() {
                    // Invariant violation: `EneHandle::run` only sends `Run`
                    // after `TurnGate::try_begin` succeeds, so reaching here
                    // means the gate is held for this new turn *and* a stream
                    // is already active. Release the gate we just acquired so
                    // a later `run()` is not stuck returning `Busy` forever;
                    // the alternative — leaving it held — would
                    // permanently wedge the single-flight gate.
                    tracing::error!(
                        component = "TurnActor",
                        "Run received while stream active; releasing turn gate \
                         (single-flight invariant violated)"
                    );
                    self.turn_gate.end();
                    return true;
                }
                // Discard any in-flight proactive decision and cancel any
                // in-flight vision summarization (the inference runs outside
                // this actor; this token is the only handle back
                // into it — see `VisionPrepared::cancel`). Replaced (not
                // just cancelled) so a vision request prepared *after* this
                // point is not pre-cancelled by the old token.
                self.proactive.on_user_turn_started();
                self.abort_proactive_decision();
                self.vision_cancel.cancel();
                self.vision_cancel = CancellationToken::new();
                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                self.terminal_emitted = Arc::new(AtomicBool::new(false));
                self.active_turn = Some(turn.clone());
                self.active_origin = crate::types::TurnOrigin::User;
                self.connectors.on_call_context(
                    &self.session.memory.session_id.to_string(),
                    Some(turn.as_str()),
                );
                drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                    status: EneStatus::Running,
                }));
                self.start_stream(
                    input,
                    turn,
                    crate::types::TurnOrigin::User,
                    true,
                    true,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
                true
            }
            EneCommand::Cancel { turn } => {
                if self.active_turn.as_ref() != Some(&turn) {
                    // Mismatch already reported by handle; ignore.
                    return true;
                }
                self.cancel_token.cancel();
                // A scheduled tool run has no stream task: the spawned task
                // observes the token at its next permission wait / dispatch
                // and reports back through `ScheduleToolFinished`, which
                // emits the terminal event, releases the gate, and records
                // the outcome. Nothing to tear down here.
                if self.active_scheduled_run.is_some() && self.stream_handle.is_none() {
                    return true;
                }

                // Cooperative join: give the stream up to 250ms to notice the
                // CancellationToken and shut down gracefully (preserving in-flight
                // session updates). If the timeout fires, hard-abort.
                if let Some(handle) = self.stream_handle.take() {
                    tokio::pin!(handle);
                    tokio::select! {
                        res = &mut handle => {
                            if let Err(e) = res {
                                tracing::warn!(
                                    component = "TurnActor",
                                    error = %e,
                                    "Stream task join failed during cooperative cancel"
                                );
                            }
                            if let Some(rx) = self.stream_session_rx.as_mut()
                                && let Ok(outcome) = rx.try_recv()
                            {
                                self.session = outcome.session;
                                self.sync_shared_session_state();
                            }
                        }
                        () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {
                            handle.as_mut().abort();
                            drop(handle.await);
                            // Try to recover the session even on hard abort:
                            // if the stream had already reached a terminal state
                            // and called session_tx.send() before being killed,
                            // the value is still available in the oneshot.
                            if let Some(rx) = self.stream_session_rx.as_mut()
                                && let Ok(outcome) = rx.try_recv()
                            {
                                self.session = outcome.session;
                                self.sync_shared_session_state();
                            }
                        }
                    }
                }
                let _ = self.stream_session_rx.take();

                // Fallback: if the stream task was hard-aborted before it could
                // record the interruption, capture the partial response here.
                // Read from the shared partial-text buffer that the stream task
                // updates live, since the session's display buffer is a pre-stream
                // snapshot that is empty after finalize.
                let partial = self.stream_partial_text.lock().clone();
                if !self.session.has_pending_interruption() && !partial.trim().is_empty() {
                    let spoken_chars = partial.chars().count();
                    self.session
                        .mark_interrupted(&turn.to_string(), &partial, spoken_chars);
                    // `mark_interrupted` records an assistant response (turn
                    // count +1); republish so the shared `turn_count` slot
                    // does not lag the actor's session.
                    self.sync_shared_session_state();
                }

                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                let cancelled_turn = turn.clone();
                if self
                    .terminal_emitted
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
                {
                    drop(self.event_tx.send(EneEvent::Terminal {
                        turn: cancelled_turn,
                        origin: self.active_origin,
                        reason: TerminalReason::Cancelled,
                    }));
                }
                self.active_turn = None;
                self.turn_gate.end();
                if self.quiet_hours_notifications_suppressed {
                    self.quiet_hours_notifications_suppressed = false;
                } else {
                    drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                        status: EneStatus::Idle,
                    }));
                }
                true
            }
            EneCommand::Shutdown => {
                // Cooperative cancel first: aux workers watching the turn's
                // token (the TTS pipeline) exit gracefully instead of being
                // force-aborted mid-flight.
                self.cancel_token.cancel();
                // Admit aux handles that arrived after the run loop's last
                // drain so shutdown aborts them rather than dropping them
                // (which would only detach, not cancel, the worker).
                self.admit_aux_handles();
                self.abort_all_join_sets();
                self.abort_proactive_decision();
                self.abort_proactive_resolution();
                self.drain_pending().await;
                // Release the single-flight gate so a `run()` that races the
                // actor's teardown is not reported `Busy` by a gate the dying
                // actor never releases. `EneHandle::run` additionally
                // checks the (now closing) command channel first and reports
                // `ActorDead`; releasing here covers the drain window before
                // the channel fully closes. Idempotent: `end()` just clears
                // the active turn.
                self.turn_gate.end();
                false
            }
            EneCommand::SetCharacter { card, reply } => {
                self.session.set_card(&card);
                *self.shared.card_name.lock() = self.session.card_name().to_string();
                *self.shared.character_card.lock() = Some(Arc::new(*card));
                self.proactive.reset_session();
                self.abort_proactive_decision();
                self.abort_proactive_resolution();
                drop(reply.send(Ok(())));
                true
            }
            EneCommand::SetGreeting { index, reply } => {
                drop(reply.send(self.apply_greeting(index)));
                true
            }
            EneCommand::UpdateProactiveObservation { observation } => {
                let seconds_since_user_input =
                    self.proactive.suppression(false).seconds_since_user_input;
                let mind = self.config.get_section::<ene_mind::MindConfig>().ok();
                self.proactive.observation = observation.clone();
                if let Some(mind) = &mind {
                    let world_observation = crate::proactive::sanitize_observation(
                        &mind.proactive,
                        observation.clone(),
                    );
                    if world_observation.captured_at_unix_ms != 0 {
                        self.proactive.world_state.push(
                            ene_mind::WorldStateSnapshot::from_observation(
                                &world_observation,
                                seconds_since_user_input,
                            ),
                            &mind.proactive.world_state,
                        );
                    }
                }
                // When screen_summary is enabled, each observe cycle (fresh
                // capture → vision) drives the decision LLM immediately.
                if mind.is_some_and(|m| m.proactive.enabled && m.proactive.sources.screen_summary) {
                    self.maybe_spawn_proactive_decision().await;
                }
                true
            }
            EneCommand::BeatPulse { bpm, intensity } => {
                drop(self.event_tx.send(EneEvent::BeatPulse { bpm, intensity }));
                true
            }
            EneCommand::ApplySettings { request, reply } => {
                let result = self.apply_settings(*request).await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "settings apply reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::GetPluginSnapshots { reply } => {
                let snapshots = self.plugin_snapshots().await;
                if reply.send(snapshots).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "plugin snapshot reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::GetArtifactSnapshot { reply } => {
                let snapshot = self.artifact_snapshot().await;
                if reply.send(snapshot).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "artifact snapshot reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::InstallArtifact {
                artifact_id,
                version,
                reply,
            } => {
                let result = self
                    .install_artifact(&artifact_id, version.as_deref())
                    .await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "artifact install reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::RollbackArtifact { artifact_id, reply } => {
                let result = self.rollback_artifact(&artifact_id).await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "artifact rollback reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::RefreshCatalog { reply } => {
                let result = self.refresh_catalog().await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "catalog refresh reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::ListPluginConfigOptions {
                plugin,
                path,
                reply,
            } => {
                let result = self.list_plugin_config_options(&plugin, &path).await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        plugin,
                        "plugin config options reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::ValidatePluginConfig {
                plugin,
                value,
                reply,
            } => {
                let result = self.validate_plugin_config(&plugin, &value).await;
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        plugin,
                        "plugin config validation reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::GetDiscoveredPlugins { reply } => {
                let config = self.config.clone();
                let mut host = self.plugin_host.lock().await;
                let discovered = host
                    .as_mut()
                    .map_or_else(Vec::new, |manager| manager.discovered_plugins(&config));
                drop(host);
                if reply.send(discovered).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "discovered-plugins reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::ListMcpStatuses { reply } => {
                let mut host = self.plugin_host.lock().await;
                let statuses = host
                    .as_mut()
                    .map_or_else(Vec::new, |manager| manager.mcp_statuses());
                drop(host);
                if reply.send(statuses).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "MCP status reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::PrepareVisionSummary {
                app_label,
                hints,
                reply,
            } => {
                self.prepare_vision_summary(app_label, hints, reply);
                true
            }
            EneCommand::StashProactiveScreenImage { data_uri } => {
                self.proactive.last_screen_image_data_uri = data_uri;
                true
            }
            EneCommand::PluginHostReconfigured {
                registry,
                plugin_tool_registries,
            } => {
                self.registry = registry;
                self.plugin_tool_registries = plugin_tool_registries;
                self.rebuild_tts_provider().await;
                true
            }
            EneCommand::RebuildTtsProvider => {
                self.rebuild_tts_provider().await;
                true
            }
            EneCommand::PluginProviderDisabled { plugin, factories } => {
                let mut guard = self.plugin_host.lock().await;
                let removal = guard
                    .as_mut()
                    .map_or_else(ene_plugin_host::ProviderFactoryRemoval::default, |host| {
                        host.remove_provider_factories_if_match(&factories)
                    });
                drop(guard);
                if removal.tts > 0 {
                    self.rebuild_tts_provider().await;
                }
                tracing::info!(
                    component = "TurnActor",
                    plugin = %plugin,
                    llm = removal.llm,
                    embedding = removal.embedding,
                    tts = removal.tts,
                    stt = removal.stt,
                    vad = removal.vad,
                    "Evicted provider factories for permanently disabled plugin"
                );
                true
            }
            EneCommand::ProbeChatCandidates { reply } => {
                self.probe_chat_candidates(reply);
                true
            }
            EneCommand::CreateChatProvider { reply } => {
                let result = create_task_chat_provider(
                    &self.config,
                    AiTaskKind::Chat,
                    self.provider_host.as_ref(),
                )
                .await
                .map(Arc::from)
                .map_err(|e| e.to_string());
                drop(reply.send(result));
                true
            }
            EneCommand::CreateSttProvider { kind, reply } => {
                let result = self
                    .provider_host
                    .create_stt_provider(&kind, &self.config)
                    .await;
                drop(reply.send(result));
                true
            }
            EneCommand::CreateVadEngine { kind, reply } => {
                let result = self
                    .provider_host
                    .create_vad_engine(&kind, &self.config)
                    .await;
                drop(reply.send(result));
                true
            }
            EneCommand::PermissionDecision {
                request_id,
                decision,
            } => {
                self.resolve_permission(request_id, decision).await;
                true
            }
            EneCommand::ResolveScheduleConfirmation {
                schedule_id,
                run_id,
                approve,
                reply,
            } => {
                let decision = if approve {
                    PermissionDecision::AllowOnce
                } else {
                    PermissionDecision::Deny
                };
                let request_id =
                    self.pending_schedule_confirmations
                        .iter()
                        .find_map(|(request_id, pending)| {
                            (pending.schedule_id == schedule_id && pending.run_id == run_id)
                                .then_some(request_id.clone())
                        });
                let resolved = if let Some(request_id) = request_id {
                    self.resolve_permission(request_id, decision).await;
                    true
                } else {
                    false
                };
                if reply.send(Ok(resolved)).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "schedule confirmation reply dropped (UI closed the window)"
                    );
                }
                true
            }
            EneCommand::BrokerApprovalRequested {
                request_id,
                plugin,
                category,
                target,
                description,
                reply,
            } => {
                let mut guard = self.pending_permissions.lock().await;
                guard.insert(request_id.clone(), reply);
                drop(guard);
                drop(self.event_tx.send(EneEvent::BrokerApprovalRequired {
                    request_id,
                    plugin,
                    category,
                    target,
                    description,
                }));
                true
            }
            EneCommand::ListPermissions { reply } => {
                let scopes = self.permission_scopes.lock().await.clone();
                drop(reply.send(scopes));
                true
            }
            EneCommand::RevokePermission { id, reply } => {
                let removed = {
                    let mut guard = self.permission_scopes.lock().await;
                    guard
                        .iter()
                        .position(|s| s.id == id)
                        .map(|pos| guard.remove(pos))
                };
                let found = removed.is_some();
                if let Some(scope) = removed {
                    self.registry
                        .revoke_pattern(&scope.action, &scope.target_pattern)
                        .await;
                    self.connectors
                        .revoke_pattern(&scope.action, &scope.target_pattern);
                }
                // A oneshot send error is `Copy` here (it's just the unsent
                // `bool`), so `drop()` would itself trip
                // `clippy::dropping_copy_types`; a dropped receiver just
                // means the caller stopped waiting.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(found);
                true
            }
            EneCommand::ResetAllPermissions { reply } => {
                let scopes = {
                    let mut guard = self.permission_scopes.lock().await;
                    std::mem::take(&mut *guard)
                };
                let count = scopes.len();
                for scope in &scopes {
                    self.registry
                        .revoke_pattern(&scope.action, &scope.target_pattern)
                        .await;
                    self.connectors
                        .revoke_pattern(&scope.action, &scope.target_pattern);
                }
                // A oneshot send error is `Copy` here (it's just the unsent
                // `usize`), so `drop()` would itself trip
                // `clippy::dropping_copy_types`; a dropped receiver just
                // means the caller stopped waiting.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(count);
                true
            }
            EneCommand::ConnectorCheck { id, reply } => {
                let connectors = Arc::clone(&self.connectors);
                self.spawn_connector_operation(id.clone(), "check", reply, move || {
                    let connectors = Arc::clone(&connectors);
                    let id = id.clone();
                    async move { connectors.check_connectivity(&id).await }
                });
                true
            }
            EneCommand::ConnectorConnect {
                id,
                credential,
                reply,
            } => {
                let connectors = Arc::clone(&self.connectors);
                let credential = Arc::new(credential);
                self.spawn_connector_operation(id.clone(), "connect", reply, move || {
                    let connectors = Arc::clone(&connectors);
                    let id = id.clone();
                    let credential = Arc::clone(&credential);
                    async move { connectors.connect(&id, credential.as_ref()).await }
                });
                true
            }
            EneCommand::ConnectorDisconnect { id, account, reply } => {
                let connectors = Arc::clone(&self.connectors);
                self.spawn_connector_operation(id.clone(), "disconnect", reply, move || {
                    let connectors = Arc::clone(&connectors);
                    let id = id.clone();
                    let account = account.clone();
                    async move { connectors.disconnect(&id, &account).await }
                });
                true
            }
            EneCommand::ConnectorGrant {
                id,
                action,
                target_pattern,
                reply,
            } => {
                let result = self.connector_grant(&id, &action, &target_pattern);
                drop(reply.send(result));
                true
            }
            EneCommand::ConnectorRevoke {
                id,
                action,
                target_pattern,
                reply,
            } => {
                let result = self.connector_revoke(&id, &action, &target_pattern);
                drop(reply.send(result));
                true
            }
            EneCommand::Undo { reply } => {
                let report = self.handle_undo().await;
                drop(reply.send(report));
                true
            }
            EneCommand::UserInputResponse {
                request_id,
                response,
            } => {
                let mut guard = self.pending_user_inputs.lock().await;
                if let Some(tx) = guard.remove(&request_id) {
                    drop(tx.send(response));
                }
                drop(guard);
                true
            }
            EneCommand::CompressContext { reply } => {
                let result = self.handle_manual_compression().await;
                drop(reply.send(result));
                true
            }
            EneCommand::GetSnapshot { reply } => {
                let history = self.session.history().to_vec();
                let snapshot = EneStateSnapshot {
                    character_card: self.session.character_card.clone(),
                    history,
                    config: self.config.clone(),
                    session_id: self.session.memory.session_id.clone(),
                    card_name: CardName::from(self.session.card_name()),
                    current_turn_count: self.session.current_turn_count() as u32,
                    session_started_at: self.session.session_started_at(),
                };
                drop(reply.send(snapshot));
                true
            }
            EneCommand::GetHistory { reply } => {
                let history = self.session.history().to_vec();
                drop(reply.send(history));
                true
            }
            EneCommand::ListTools { reply } => {
                let mut tools = self.registry.list_tools();
                tools.push(crate::streaming::search_tools_spec());
                drop(reply.send(tools));
                true
            }
            EneCommand::SearchTools { query, reply } => {
                if !admit_task(
                    &mut self.search_tasks,
                    self.task_caps.search_cap,
                    "SearchTools",
                    Some(query.clone()),
                    &self.diag_tx,
                ) {
                    drop(reply.send(Err(EneRuntimeError::Busy {
                        queue_depth: self.task_caps.search_cap,
                    })));
                    return true;
                }
                let registry = self.registry.clone();
                let tool_rag = self.tool_rag.clone();
                let card_name = self.session.card_name().to_string();
                self.search_tasks.spawn(async move {
                    let result = if let Some(rag) = tool_rag {
                        let all_tools = registry.list_tools();
                        let profiles = registry.list_rag_profiles();
                        if let Err(e) = rag.ensure_index(&all_tools, &profiles).await {
                            tracing::warn!(component = "ToolRag", error = %e, "ensure_index failed");
                        }
                        rag.select(&query, &card_name).await
                    } else {
                        let query_lower = query.to_lowercase();
                        registry
                            .list_tools()
                            .into_iter()
                            .filter(|t| {
                                t.name.as_str().to_lowercase().contains(&query_lower)
                                    || t.description.to_lowercase().contains(&query_lower)
                            })
                            .collect()
                    };
                    drop(reply.send(Ok(result)));
                });
                true
            }
            EneCommand::CallTool {
                name,
                arguments,
                turn,
                reply,
            } => {
                if !admit_task(
                    &mut self.call_tool_tasks,
                    self.task_caps.call_tool_cap,
                    "CallTool",
                    Some(name.clone()),
                    &self.diag_tx,
                ) {
                    drop(reply.send(Err(EneRuntimeError::Busy {
                        queue_depth: self.task_caps.call_tool_cap,
                    })));
                    return true;
                }
                let registry = self.registry.clone();
                let tool_rag = self.tool_rag.clone();
                let session_id = self.session.memory.session_id.to_string();
                let card_name = self.session.card_name().to_string();
                self.call_tool_tasks.spawn(async move {
                    // Direct calls (`PublicApi::call_tool`) carry no turn;
                    // a unique synthetic turn keeps the per-turn approval
                    // expiry in plugins running uniformly, so a direct call
                    // can never inherit an approval granted in a chat turn.
                    let context = ene_plugin_proto::CallContext {
                        conversation_id: session_id,
                        turn_id: turn.map_or_else(
                            || format!("direct:{}", uuid::Uuid::new_v4()),
                            |turn| turn.to_string(),
                        ),
                    };
                    let result: Result<String, EneRuntimeError> = if name == "system.search_tools" {
                        let query = serde_json::from_str::<serde_json::Value>(&arguments)
                            .ok()
                            .and_then(|v| v.get("query").and_then(|q| q.as_str()).map(String::from))
                            .unwrap_or_default();
                        crate::streaming::execute_system_search_tool(
                            registry.as_ref(),
                            tool_rag.as_deref(),
                            &query,
                            &card_name,
                        )
                        .await
                        .map_err(EneRuntimeError::from)
                    } else {
                        registry
                            .call_tool(&name, &arguments, Some(&context))
                            .await
                            .map(|r| r.text_for_llm())
                            .map_err(EneRuntimeError::from)
                    };
                    drop(reply.send(result));
                });
                true
            }
            EneCommand::ScheduleFire {
                schedule_id,
                scheduled_at,
            } => {
                self.handle_schedule_fire(schedule_id, scheduled_at).await;
                true
            }
            EneCommand::ScheduleConfirmationTimeout {
                request_id,
                schedule_id,
                run_id,
            } => {
                self.pending_schedule_confirmations.remove(&request_id);
                self.finish_scheduled_run(
                    schedule_id,
                    run_id,
                    ScheduleRunStatus::TimedOut,
                    Some("confirmation timed out".to_string()),
                )
                .await;
                true
            }
            EneCommand::ScheduleToolFinished {
                schedule_id,
                run_id,
                turn,
                tool_name,
                denied,
                result,
            } => {
                if self.active_turn.as_ref() != Some(&turn) {
                    return true; // stale outcome for a turn already finished
                }
                self.active_scheduled_run = None;
                self.active_turn = None;
                self.active_origin = crate::types::TurnOrigin::User;
                let (mut status, mut error, terminal_reason, result_text) = match result {
                    Ok(text) => (ScheduleRunStatus::Success, None, TerminalReason::Done, text),
                    Err(message) => (
                        ScheduleRunStatus::Failed,
                        Some(message.clone()),
                        TerminalReason::Failed {
                            message: message.clone(),
                        },
                        format!("Error executing tool: {message}"),
                    ),
                };
                if denied {
                    // A denied permission prompt is a terminal, no-retry
                    // outcome: the user already said no once, so arming a
                    // retry would just re-open the dialog.
                    status = ScheduleRunStatus::Denied;
                    error = Some("Permission denied by user".to_string());
                }
                drop(self.event_tx.send(EneEvent::ToolCallResult {
                    turn: turn.clone(),
                    origin: crate::types::TurnOrigin::Scheduled,
                    name: tool_name,
                    result: result_text,
                }));
                streaming::emit_terminal(
                    &self.event_tx,
                    &self.terminal_emitted,
                    &turn,
                    crate::types::TurnOrigin::Scheduled,
                    terminal_reason,
                );
                self.finish_scheduled_run(schedule_id, run_id, status, error)
                    .await;
                self.turn_gate.end();
                drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                    status: EneStatus::Idle,
                }));
                true
            }
            EneCommand::AddSchedule { new, reply } => {
                let result = match &self.concrete_store {
                    Some(store) => store
                        .insert_schedule(&new, (self.scheduler_clock)())
                        .await
                        .map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                // A oneshot `Sender<Result<..>>::send` error is `Copy` (the
                // unsent `Result`), so `drop()` would itself trip
                // `clippy::dropping_copy_types`.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(result);
                self.notify_scheduler();
                true
            }
            EneCommand::UpdateSchedule { id, new, reply } => {
                let result = match &self.concrete_store {
                    Some(store) => store
                        .update_schedule(id, &new, (self.scheduler_clock)())
                        .await
                        .map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                if reply.send(result).is_err() {
                    tracing::debug!(
                        component = "TurnActor",
                        "schedule update reply dropped (UI closed the window)"
                    );
                }
                self.notify_scheduler();
                true
            }
            EneCommand::ListSchedules { reply } => {
                let result = match &self.concrete_store {
                    Some(store) => store.list_schedules().await.map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(result);
                true
            }
            EneCommand::ListScheduleRuns {
                schedule_id,
                limit,
                reply,
            } => {
                let result = match &self.concrete_store {
                    Some(store) => store
                        .list_runs(schedule_id, limit)
                        .await
                        .map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(result);
                true
            }
            EneCommand::DeleteSchedule { schedule_id, reply } => {
                let result = match &self.concrete_store {
                    Some(store) => store
                        .delete_schedule(schedule_id)
                        .await
                        .map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(result);
                self.notify_scheduler();
                true
            }
            EneCommand::SetScheduleEnabled {
                schedule_id,
                enabled,
                reply,
            } => {
                let result = match &self.concrete_store {
                    Some(store) => store
                        .set_schedule_enabled(schedule_id, enabled, (self.scheduler_clock)())
                        .await
                        .map_err(EneRuntimeError::from),
                    None => Err(EneRuntimeError::StoreRequired),
                };
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(result);
                self.notify_scheduler();
                true
            }
            EneCommand::CancelDeferredTool {
                tool_name,
                task_id,
                reply,
            } => {
                if !admit_task(
                    &mut self.call_tool_tasks,
                    self.task_caps.call_tool_cap,
                    "CallTool",
                    Some(format!("cancel:{tool_name}:{task_id}")),
                    &self.diag_tx,
                ) {
                    // Best-effort like the success path below: report "not
                    // cancelled" rather than growing the queue. A oneshot
                    // send error is `Copy` here (it's just the unsent
                    // `bool`), so `drop()` would itself trip
                    // `clippy::dropping_copy_types`.
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                    )]
                    let _ = reply.send(false);
                    return true;
                }
                let registry = self.registry.clone();
                self.call_tool_tasks.spawn(async move {
                    registry.cancel_deferred(&tool_name, &task_id).await;
                    // The tool-side cancel is best-effort; report success
                    // optimistically since we cannot distinguish "cancelled"
                    // from "already finished" without a follow-up poll. A
                    // oneshot send error is `Copy` here (it's just the
                    // unsent `bool`), so `drop()` would itself trip
                    // `clippy::dropping_copy_types`.
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                    )]
                    let _ = reply.send(true);
                });
                true
            }
            EneCommand::InvalidateToolIndex => {
                self.tool_rag = None;
                true
            }
            EneCommand::WorkspaceStartSync { reply } => {
                let result = self.spawn_workspace_sync();
                drop(reply.send(result));
                true
            }
            EneCommand::WorkspaceCancelSync => {
                if let Some(token) = &self.workspace_cancel {
                    token.cancel();
                }
                true
            }
            EneCommand::WorkspaceStatus { reply } => {
                let config = self
                    .config
                    .get_section::<ene_rag::WorkspaceRagConfig>()
                    .unwrap_or_default();
                let state = self.workspace_state.lock().clone();
                let mut view = crate::workspace::WorkspaceStatusView {
                    enabled: config.enabled,
                    folders: config.folders.clone(),
                    indexed_files: 0,
                    indexed_chunks: 0,
                    in_progress: state.in_progress,
                    progress: state.progress.clone(),
                    last_report: state.last_report.clone(),
                    last_error: state.last_error.clone(),
                };
                if let Some(indexer) = &self.workspace_indexer
                    && let Ok(status) = indexer.index_status().await
                {
                    view.indexed_files = status.indexed_files;
                    view.indexed_chunks = status.indexed_chunks;
                }
                drop(reply.send(view));
                true
            }
            EneCommand::WorkspaceSearch {
                query,
                limit,
                reply,
            } => {
                let config = self
                    .config
                    .get_section::<ene_rag::WorkspaceRagConfig>()
                    .unwrap_or_default();
                let result = if config.enabled {
                    match self.workspace_indexer.clone() {
                        Some(indexer) => indexer
                            .search(&config, &query, limit)
                            .await
                            .map_err(EneRuntimeError::from),
                        None => Err(EneRuntimeError::MindPrerequisite(
                            "workspace indexer unavailable",
                        )),
                    }
                } else {
                    Err(crate::workspace::WorkspaceIndexError::Disabled.into())
                };
                drop(reply.send(result));
                true
            }
            EneCommand::ResolveCandidate {
                id,
                status,
                turn,
                reply,
            } => {
                // Candidate mutations are serialized by the actor. The
                // shared recall cache is invalidated only after a successful
                // approve or reject so a stale pending or typed-memory entry
                // cannot survive the mutation.
                let Some(store) = self.concrete_store.clone() else {
                    drop(reply.send(Err(crate::public_api::PublicApiError::Internal {
                        message: "Memory store is not enabled".to_string(),
                    })));
                    return true;
                };
                let result = match status {
                    ene_store::PendingCandidateStatus::Approved => {
                        let approved = store.approve_pending_candidate(id).await;
                        if let Ok(memory_id) = approved
                            && let Ok(mind_cfg) = self.config.get_section::<ene_mind::MindConfig>()
                            && mind_cfg.memory.reflection.enabled
                            && let Ok(Some(candidate)) = store.get_pending_candidate(id).await
                        {
                            // The approval insert bypasses the arbiter, so the
                            // deferred rating is written back here to keep the
                            // approved memory in the self-reflection loop.
                            ene_mind::memory_writer::reflection::record_approved_outcome(
                                store.as_ref(),
                                &candidate,
                                memory_id,
                            )
                            .await;
                        }
                        approved
                            .map(|_| ())
                            .map_err(crate::public_api::PublicApiError::from)
                    }
                    ene_store::PendingCandidateStatus::Rejected => store
                        .resolve_pending_candidate(id, false)
                        .await
                        .map_err(crate::public_api::PublicApiError::from),
                    ene_store::PendingCandidateStatus::Pending => {
                        Err(crate::public_api::PublicApiError::Invalid {
                            message: format!("cannot resolve pending candidate {id} to 'pending'"),
                        })
                    }
                };
                if result.is_ok() {
                    if let Some(cache) = &self.session.memory.recall_cache {
                        cache.invalidate_character(self.session.card_name());
                    }
                    drop(self.lifecycle_tx.send(LifecycleEvent::CandidateChanged {
                        id,
                        status,
                        turn,
                    }));
                }
                drop(reply.send(result));
                true
            }
            EneCommand::EditCandidate {
                id,
                edit,
                turn,
                reply,
            } => {
                let Some(store) = self.concrete_store.clone() else {
                    drop(reply.send(Err(crate::public_api::PublicApiError::Internal {
                        message: "Memory store is not enabled".to_string(),
                    })));
                    return true;
                };
                let result = store
                    .edit_pending_candidate(id, edit)
                    .await
                    .map(|_| ())
                    .map_err(crate::public_api::PublicApiError::from);
                if result.is_ok() {
                    if let Some(cache) = &self.session.memory.recall_cache {
                        cache.invalidate_character(self.session.card_name());
                    }
                    drop(self.lifecycle_tx.send(LifecycleEvent::CandidateChanged {
                        id,
                        status: ene_store::PendingCandidateStatus::Pending,
                        turn,
                    }));
                }
                drop(reply.send(result));
                true
            }
            EneCommand::EditMemory {
                id,
                edit,
                turn,
                reply,
            } => {
                let Some(store) = self.concrete_store.clone() else {
                    drop(reply.send(Err(crate::public_api::PublicApiError::Internal {
                        message: "Memory store is not enabled".to_string(),
                    })));
                    return true;
                };
                let owner = Some(self.config.user_name.clone());
                let result = match store.update_typed_memory(id, &edit, owner.as_deref()).await {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(crate::public_api::PublicApiError::NotFound {
                        message: format!("memory {id} not found"),
                    }),
                    Err(error) => Err(crate::public_api::PublicApiError::from(error)),
                };
                if result.is_ok() {
                    if let Some(cache) = &self.session.memory.recall_cache {
                        cache.invalidate_character(self.session.card_name());
                    }
                    self.spawn_memory_reembed(id, &edit).await;
                    drop(self.lifecycle_tx.send(LifecycleEvent::MemoryLedgerChanged {
                        id,
                        action: MemoryLedgerChange::Edited,
                        turn,
                    }));
                }
                drop(reply.send(result));
                true
            }
            EneCommand::SetMemorySalience {
                id,
                salience,
                turn,
                reply,
            } => {
                let Some(store) = self.concrete_store.clone() else {
                    drop(reply.send(Err(crate::public_api::PublicApiError::Internal {
                        message: "Memory store is not enabled".to_string(),
                    })));
                    return true;
                };
                let result = match store.set_memory_salience(id, salience).await {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(crate::public_api::PublicApiError::NotFound {
                        message: format!("memory {id} not found"),
                    }),
                    Err(error) => Err(crate::public_api::PublicApiError::from(error)),
                };
                if result.is_ok() {
                    if let Some(cache) = &self.session.memory.recall_cache {
                        cache.invalidate_character(self.session.card_name());
                    }
                    drop(self.lifecycle_tx.send(LifecycleEvent::MemoryLedgerChanged {
                        id,
                        action: MemoryLedgerChange::SalienceAdjusted,
                        turn,
                    }));
                }
                drop(reply.send(result));
                true
            }
            EneCommand::SetCcv3MemoryHash { hash, reply } => {
                self.session.memory.ccv3_memory_hash = Some(hash);
                // A oneshot send error is `Copy` here (it's just the unsent
                // `()`), so `drop()` would itself trip
                // `clippy::dropping_copy_types`; a dropped receiver just
                // means the caller stopped waiting.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(());
                true
            }
            #[cfg(test)]
            EneCommand::TestInjectPanicAfterMutations {
                request_id,
                permission_tx,
            } => {
                self.pending_permissions
                    .lock()
                    .await
                    .insert(request_id, permission_tx);
                self.permission_scopes
                    .lock()
                    .await
                    .push(crate::streaming::PermissionScope {
                        id: 999_999,
                        action: "test.action".to_string(),
                        target_pattern: "test-pattern".to_string(),
                        grant_type: crate::streaming::GrantType::Session,
                        granted_at: chrono::Utc::now(),
                    });
                self.undo_stack.lock().await.record(
                    "filesystem.write",
                    "test-turn",
                    vec!["/test/path".to_string()],
                );
                panic!("induced panic after mutating shared actor state");
            }
            #[cfg(test)]
            EneCommand::TestSpawnSlowBgTask { reply } => {
                // Admit a long-running task into `bg_command_tasks` to
                // simulate a heavy background command (GGUF load / plugin
                // host restart) being in flight. The actor loop must
                // keep processing subsequent commands while this sleeps.
                if admit_task(
                    &mut self.bg_command_tasks,
                    self.task_caps.bg_command_cap,
                    "BgCommand",
                    Some("TestSlowBgTask".to_string()),
                    &self.diag_tx,
                ) {
                    self.bg_command_tasks.spawn(async {
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    });
                }
                // A oneshot `Sender<()>::send` error is `Copy` (the unsent
                // `()`), so `drop()` would trip `clippy::dropping_copy_types`.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(());
                true
            }
        }
    }

    async fn start_stream(
        &mut self,
        user_input: String,
        turn: TurnId,
        origin: crate::types::TurnOrigin,
        record_user_message: bool,
        allow_tools: bool,
        runtime_directive: Option<String>,
        proactive_screen_image: Option<String>,
        proactive_topic: Option<String>,
        generation_timeout: Option<std::time::Duration>,
    ) -> bool {
        if origin != crate::types::TurnOrigin::Proactive {
            self.quiet_hours_notifications_suppressed = false;
        }
        // Create the provider before mutating history so a failed open leaves
        // the session unchanged.
        let provider = match if origin == crate::types::TurnOrigin::Proactive {
            self.create_proactive_provider().await
        } else {
            self.create_provider().await
        } {
            Ok(p) => p,
            Err(e) => {
                if self
                    .terminal_emitted
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
                {
                    drop(self.event_tx.send(EneEvent::Terminal {
                        turn,
                        origin,
                        reason: TerminalReason::Failed {
                            message: e.to_string(),
                        },
                    }));
                }
                self.active_turn = None;
                self.turn_gate.end();
                if origin == crate::types::TurnOrigin::Proactive {
                    // No stream will run, so no completion arm will consume it.
                    self.proactive.last_decision = None;
                }
                if self.quiet_hours_notifications_suppressed {
                    self.quiet_hours_notifications_suppressed = false;
                } else {
                    drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                        status: EneStatus::Idle,
                    }));
                }
                return false;
            }
        };

        self.apply_pending_compression().await;

        if record_user_message {
            self.maybe_split_session_on_timeout();
            self.session.record_user_input();
            self.session.add_user_message(&user_input);
            self.sync_shared_session_state();
            self.check_and_trigger_compression().await;
        }

        // Snapshot whether a rolling-compression summary is still pending for
        // this session. Prompt packing reads it to synchronously detach the
        // oldest span from the prompt-visible history while the summary is in
        // flight, instead of shedding sections.
        let compression_pending = self.context.has_pending();

        let config = self.config.clone();
        let session = self.session.clone();
        let embedder = self.session.memory.embedding_provider.clone();
        let registry = self.registry.clone();
        let tool_rag = self.tool_rag.clone();
        let event_tx = self.event_tx.clone();
        let audio_tx = self.audio_tx.clone();
        let diag_tx = self.diag_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let pending_permissions = self.pending_permissions.clone();
        let pending_user_inputs = self.pending_user_inputs.clone();
        let permission_scopes = self.permission_scopes.clone();
        let undo_stack = self.undo_stack.clone();
        let terminal_emitted = self.terminal_emitted.clone();
        let turn_for_stream = turn.clone();
        let classifier_tx = self.classifier_tx.clone();
        let memory_writer_tx = self.memory_writer_tx.clone();
        let deferred_tool_tx = self.deferred_tool_tx.clone();
        let aux_task_tx = self.aux_task_tx.clone();
        let tts_provider = if origin == crate::types::TurnOrigin::Proactive {
            let mind = self
                .config
                .get_section::<ene_mind::MindConfig>()
                .unwrap_or_default();
            let quiet =
                evaluate_quiet_hours(&mind.proactive.quiet_hours, (self.quiet_hours_clock)());
            let suppress = mind.proactive.quiet_hours.suppress;
            if crate::proactive::quiet_hours_suppresses_tts(&quiet, suppress) {
                tracing::info!(
                    component = "Proactive",
                    event = "quiet_hours_tts_suppressed",
                    weekday = %quiet.weekday,
                    local_time = %quiet.local_time,
                    "Proactive TTS suppressed by quiet hours"
                );
            }
            self.proactive_tts_provider(&quiet, suppress)
        } else {
            self.tts_provider.clone()
        };
        // Reset the shared partial-text buffer for this turn and hand a clone
        // to the stream task so a hard-abort can recover streamed text.
        self.stream_partial_text.lock().clear();
        let partial_text = Arc::clone(&self.stream_partial_text);
        self.active_origin = origin;

        drop(self.event_tx.send(EneEvent::TurnStarted {
            turn: turn_for_stream.clone(),
            origin,
        }));

        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let concrete_store_for_stream = self.concrete_store.clone();
        let provider_host = Arc::clone(&self.provider_host);
        let handle = tokio::spawn(async move {
            let panic_event_tx = event_tx.clone();
            let panic_terminal_emitted = Arc::clone(&terminal_emitted);
            let panic_turn = turn_for_stream.clone();
            let stream_turn = panic_turn.clone();
            let outcome =
                match futures::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
                    streaming::run_stream_cognitive(streaming::StreamContext {
                        config,
                        session,
                        user_input,
                        embedder,
                        registry,
                        tool_rag,
                        provider,
                        provider_host,
                        event_tx,
                        audio_tx,
                        diag_tx,
                        cancel_token,
                        pending_permissions,
                        pending_user_inputs,
                        permission_scopes,
                        undo_stack,
                        terminal_emitted: Arc::clone(&panic_terminal_emitted),
                        turn: stream_turn,
                        origin,
                        allow_tools,
                        runtime_directive,
                        proactive_screen_image,
                        proactive_topic,
                        generation_timeout,
                        classifier_tx,
                        memory_writer_tx,
                        deferred_tool_tx,
                        aux_task_tx,
                        tts_provider,
                        partial_text,
                        compression_pending,
                        concrete_store: concrete_store_for_stream,
                    })
                    .await
                }))
                .await
                {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        // The stream task panicked. The actor's own panic
                        // isolation only protects the command loop from panics in
                        // *command handlers* — this is a separately spawned task,
                        // so its panic needs its own catch_unwind or a hard-aborted
                        // turn would otherwise hang forever waiting for `Terminal`.
                        let msg = if let Some(s) = e.downcast_ref::<String>() {
                            s.clone()
                        } else if let Some(s) = e.downcast_ref::<&str>() {
                            (*s).to_string()
                        } else {
                            "unknown panic".to_string()
                        };
                        tracing::error!(
                            component = "StreamTask",
                            error = %msg,
                            "Stream task panicked; emitting fallback Terminal"
                        );
                        streaming::emit_terminal(
                            &panic_event_tx,
                            &panic_terminal_emitted,
                            &panic_turn,
                            origin,
                            TerminalReason::Failed {
                                message: format!("stream task panicked: {msg}"),
                            },
                        );
                        return;
                    }
                };
            drop(session_tx.send(outcome));
        });
        self.stream_handle = Some(handle);
        true
    }

    async fn create_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        let ai_config = self
            .config
            .get_section::<ene_ai::AiConfig>()
            .unwrap_or_default();

        // Fast path: failover disabled → use the configured chat task directly.
        if !ai_config.fallback.enabled {
            return create_task_chat_provider(
                &self.config,
                AiTaskKind::Chat,
                self.provider_host.as_ref(),
            )
            .await
            .map(Arc::from)
            .map_err(EneRuntimeError::from);
        }

        // Failover path: probe candidates in priority order and pick the first
        // healthy one. Probes send no user data.
        let candidates = ai_config.resolve_chat_candidates();
        if candidates.is_empty() {
            return create_task_chat_provider(
                &self.config,
                AiTaskKind::Chat,
                self.provider_host.as_ref(),
            )
            .await
            .map(Arc::from)
            .map_err(EneRuntimeError::from);
        }

        let timeout = std::time::Duration::from_millis(ai_config.fallback.health_check_timeout_ms);
        let selection = ene_ai::select_healthy_chat(
            &candidates,
            &self.health_monitor,
            self.provider_host.as_ref(),
            &self.config,
            timeout,
        )
        .await
        .ok_or_else(|| {
            EneRuntimeError::from(ene_ai::LlmProviderError::Provider(
                "no chat provider candidates available".to_string(),
            ))
        })?;

        // Emit a health diagnostic for every probed candidate so the UI can
        // show per-provider status without polling.
        for report in self.health_monitor.all_reports() {
            drop(self.diag_tx.send(DiagnosticEvent::ProviderHealth {
                provider: report.provider.clone(),
                status: report.status.status_code().to_string(),
                latency_ms: report.latency_ms,
                detail: report.error.clone(),
            }));
        }

        if selection.fell_back {
            let reason = selection
                .skipped
                .iter()
                .map(|(p, r)| format!("{p}: {r}"))
                .collect::<Vec<_>>()
                .join("; ");
            drop(
                self.diag_tx.send(DiagnosticEvent::ProviderFallback {
                    from: candidates
                        .first()
                        .map_or_else(String::new, |c| c.provider.clone()),
                    to: selection.candidate.provider.clone(),
                    reason,
                }),
            );
        }

        // Route the selected candidate through the plugin-backed provider
        // registry by kind; the candidate's provider name, model, and token
        // cap are forwarded so the produced provider matches the probed
        // candidate exactly.
        let candidate = &selection.candidate;
        let mut task = ene_ai::TaskRef {
            provider: candidate.provider.clone(),
            max_tokens: candidate.max_tokens,
            ..ene_ai::TaskRef::default()
        };
        task.model = Some(candidate.model.clone());
        let provider =
            ene_ai::create_chat_provider_for_task(&self.config, &task, self.provider_host.as_ref())
                .await
                .map_err(EneRuntimeError::from)?;
        Ok(Arc::from(provider))
    }

    async fn create_proactive_provider(
        &self,
    ) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        create_task_chat_provider(
            &self.config,
            AiTaskKind::Proactive,
            self.provider_host.as_ref(),
        )
        .await
        .map(Arc::from)
        .map_err(EneRuntimeError::from)
    }

    /// Probe every chat failover candidate through the provider host.
    ///
    /// Runs in a background task so slow probes cannot stall the actor loop.
    /// Probes are fresh and non-caching (the shared failover monitor is not
    /// warmed). Used by the CLI `/doctor` fallback check.
    fn probe_chat_candidates(&self, reply: oneshot::Sender<Vec<ene_ai::ProviderHealthReport>>) {
        let ai_config = self
            .config
            .get_section::<ene_ai::AiConfig>()
            .unwrap_or_default();
        let candidates = ai_config.resolve_chat_candidates();
        let timeout = std::time::Duration::from_millis(ai_config.fallback.health_check_timeout_ms);
        let provider_host = Arc::clone(&self.provider_host);
        let config = self.config.clone();
        let diag_tx = self.diag_tx.clone();
        tokio::spawn(async move {
            // Fresh probes of every candidate: unlike failover selection,
            // this must not stop at the first healthy provider, and it must
            // not warm the turn-path cache.
            let reports = if ai_config.fallback.enabled {
                ene_ai::probe_chat_candidates(&candidates, provider_host.as_ref(), &config, timeout)
                    .await
            } else {
                Vec::new()
            };
            for report in &reports {
                drop(diag_tx.send(DiagnosticEvent::ProviderHealth {
                    provider: report.provider.clone(),
                    status: report.status.status_code().to_string(),
                    latency_ms: report.latency_ms,
                    detail: report.error.clone(),
                }));
            }
            drop(reply.send(reports));
        });
    }

    // ── Undo management ──

    /// Undo the most recent reversible tool operation.
    ///
    /// Only reversible filesystem mutations are recorded on the stack
    /// (irreversible actions are warned about at execution time but never
    /// pushed), so popping the newest entry and invoking the owning fs
    /// tool's `utility.undo` action rolls it back.
    async fn handle_undo(&mut self) -> crate::undo::UndoReport {
        use crate::undo::UndoReport;

        let popped = { self.undo_stack.lock().await.pop_reversible() };
        let Some(entry) = popped else {
            return UndoReport::NothingToUndo;
        };

        match self.registry.call_tool("utility.undo", "{}", None).await {
            Ok(output) => UndoReport::Reverted {
                metadata: entry.metadata,
                output: output.text_for_llm(),
            },
            Err(e) => UndoReport::Failed {
                metadata: entry.metadata,
                error: e.to_string(),
            },
        }
    }

    // ── Connector framework ──

    /// Spawns a connector lifecycle operation (check / connect / disconnect)
    /// as a background task and replies when it resolves.
    ///
    /// The task — not the actor loop — awaits the permission decision:
    /// `PermissionDecision` is delivered through the mailbox, so an inline
    /// await would deadlock the loop (the same reason tool prompt
    /// resolution runs in stream tasks).
    fn spawn_connector_operation<T, F, Fut>(
        &mut self,
        id: ene_connector::ConnectorId,
        op: &'static str,
        reply: oneshot::Sender<Result<T, ene_connector::ConnectorError>>,
        run: F,
    ) where
        T: Send + 'static,
        F: FnMut() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, ene_connector::ConnectorError>> + Send,
    {
        let connectors = Arc::clone(&self.connectors);
        let event_tx = self.event_tx.clone();
        let pending_permissions = self.pending_permissions.clone();
        let permission_scopes = self.permission_scopes.clone();
        let concrete_store = self.concrete_store.clone();
        let session_id = self.session.memory.session_id.to_string();
        let active_turn = self.active_turn.clone();
        let prompt_timeout_ms = self
            .config
            .get_section::<ene_plugin_host::PluginConfig>()
            .map_or(300_000, |cfg| cfg.permission_prompt_timeout_ms);
        self.aux_tasks.spawn(async move {
            let result = crate::connectors::run_connector_operation(
                &connectors,
                &id,
                op,
                &event_tx,
                &pending_permissions,
                &permission_scopes,
                prompt_timeout_ms,
                active_turn,
                concrete_store,
                session_id,
                run,
            )
            .await;
            drop(reply.send(result));
        });
    }

    /// Records a per-action connector grant (explicit user command, audited).
    fn connector_grant(
        &self,
        id: &ene_connector::ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<(), ene_connector::ConnectorError> {
        let result = self.connectors.grant(id, action, target_pattern);
        crate::connectors::record_connector_audit(
            self.concrete_store.as_ref(),
            &self.session.memory.session_id.to_string(),
            self.active_turn.as_ref(),
            &format!("connector.{id}.grant"),
            action,
            target_pattern,
            ene_store::AuditDecision::NotRequired,
            result.is_ok(),
        );
        result
    }

    /// Removes a per-action connector grant (explicit user command, audited).
    fn connector_revoke(
        &self,
        id: &ene_connector::ConnectorId,
        action: &str,
        target_pattern: &str,
    ) -> Result<bool, ene_connector::ConnectorError> {
        let result = self.connectors.revoke(id, action, target_pattern);
        crate::connectors::record_connector_audit(
            self.concrete_store.as_ref(),
            &self.session.memory.session_id.to_string(),
            self.active_turn.as_ref(),
            &format!("connector.{id}.revoke"),
            action,
            target_pattern,
            ene_store::AuditDecision::NotRequired,
            result.is_ok(),
        );
        result
    }

    // ── Split management ──

    /// Opens the session with the greeting at `index` (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`).
    ///
    /// Only valid before the first message: a greeting mid-conversation
    /// would insert an assistant turn the character never produced.
    fn apply_greeting(&mut self, index: u32) -> Result<String, crate::public_api::PublicApiError> {
        use crate::public_api::PublicApiError;

        let Some(card) = self.session.character_card.as_ref() else {
            return Err(PublicApiError::Invalid {
                message: "no character card loaded".to_string(),
            });
        };
        if self.active_turn.is_some() {
            return Err(PublicApiError::Invalid {
                message: "a turn is in progress; wait for it to finish before choosing a greeting"
                    .to_string(),
            });
        }
        if !self.session.history().is_empty() {
            return Err(PublicApiError::Invalid {
                message: "greeting can only be chosen before the first message".to_string(),
            });
        }

        let index_usize = usize::try_from(index).map_err(|_| PublicApiError::Invalid {
            message: format!("greeting index {index} is out of range"),
        })?;
        let text = if index_usize == 0 {
            card.data.first_mes.clone()
        } else {
            match card
                .data
                .alternate_greetings
                .get(index_usize.saturating_sub(1))
            {
                Some(text) => text.clone(),
                None => {
                    return Err(PublicApiError::Invalid {
                        message: format!("greeting index {index} is out of range"),
                    });
                }
            }
        };
        if text.trim().is_empty() {
            return Err(PublicApiError::Invalid {
                message: format!("no greeting at index {index}"),
            });
        }

        let user_name = self.config.user_name.clone();
        let expanded =
            ene_card::expand_cbs_macros(&text, card.data.get_character_name(), &user_name);
        self.session.apply_greeting(&expanded, index);
        if let Some(store) = &self.concrete_store {
            ene_store::MemoryStore::spawn_insert_log(
                store,
                self.session.memory.session_id.as_str(),
                self.session.card_name(),
                "assistant",
                &expanded,
            );
        }
        Ok(expanded)
    }

    /// Starts a new session: resets the session and publishes the new id /
    /// started-at / turn count to the mailbox-free shared state.
    ///
    /// This is the single place a session split (currently only the
    /// inactivity-timeout path) may occur, so it is also the
    /// single place the shared [`SharedActorState`] session slots are
    /// refreshed — [`crate::EneHandle::session_id`] / `session_started_at` /
    /// `turn_count` must reflect the fresh session immediately.
    fn split_session(&mut self) -> SessionId {
        let new_id = self.session.reset_session();
        self.sync_shared_session_state();
        new_id
    }

    /// Publishes the current session id / started-at / turn count to the
    /// mailbox-free shared state as a single atomic snapshot. Idempotent;
    /// called after a session split, after a user input is recorded, and after
    /// a stream outcome is applied (the stream task records the assistant
    /// response on its own session clone).
    fn sync_shared_session_state(&self) {
        *self.shared.session.write() = super::SharedSessionState {
            session_id: self.session.memory.session_id.clone(),
            session_started_at: self.session.session_started_at(),
            turn_count: self.session.current_turn_count() as u32,
        };
    }

    /// Publishes the current config to the mailbox-free shared state.
    ///
    /// Called after every config mutation ([`EneCommand::ApplySettings`],
    /// character swaps) so [`crate::EneHandle::config`] stays consistent with
    /// the actor's authoritative copy. Read-only sharing: only the actor
    /// writes, and only under a write lock.
    fn sync_shared_config(&self) {
        let cfg = Arc::new(self.config.clone());
        *self.shared.config.write() = cfg;
    }

    /// Applies a unified settings draft: diff against the live config, write
    /// changed sections, react per section, and report impact.
    ///
    /// On a section-write failure the config is rolled back to the pre-apply
    /// copy and an error is returned; the UI can then re-sync its draft from
    /// the persisted values.
    async fn apply_settings(
        &mut self,
        request: crate::settings::SettingsApplyRequest,
    ) -> Result<crate::settings::SettingsApplyResult, EneRuntimeError> {
        if request
            .base_revision
            .is_some_and(|base| base != self.settings_revision)
        {
            tracing::warn!(
                component = "TurnActor",
                base = request.base_revision,
                current = self.settings_revision,
                "rejecting stale settings draft; the UI must re-sync"
            );
            return Ok(crate::settings::SettingsApplyResult {
                revision: request.revision,
                current_revision: self.settings_revision,
                conflicted: true,
                applied_sections: std::collections::BTreeSet::new(),
                impact: crate::settings::SettingsImpact::default(),
                errors: Vec::new(),
            });
        }
        let prev = self.config.clone();
        let changed = crate::settings::changed_sections(&prev, &request.config);
        let mut applied = std::collections::BTreeSet::new();
        let mut errors = Vec::new();

        for key in &changed {
            let write = match key.as_str() {
                "character" => {
                    self.config.character.clone_from(&request.config.character);
                    Ok(())
                }
                "user_name" => {
                    self.config.user_name.clone_from(&request.config.user_name);
                    Ok(())
                }
                "runtime_rules" => {
                    self.config
                        .runtime_rules
                        .clone_from(&request.config.runtime_rules);
                    Ok(())
                }
                "user_persona" => {
                    self.config
                        .user_persona
                        .clone_from(&request.config.user_persona);
                    Ok(())
                }
                section => {
                    if let Some(value) = request.config.section_value(section) {
                        self.config.set_section_value(section, value)
                    } else {
                        self.config.remove_section(section);
                        Ok(())
                    }
                }
            };
            match write {
                Ok(()) => {
                    applied.insert(key.clone());
                }
                Err(e) => errors.push(format!("{key}: {e}")),
            }
        }

        if !errors.is_empty() {
            self.config = prev;
            self.sync_shared_config();
            return Err(EneRuntimeError::Config(
                ene_config::EneConfigError::GenericConfigError(format!(
                    "Failed to apply settings sections: {}",
                    errors.join("; ")
                )),
            ));
        }

        let mut impact = crate::settings::SettingsImpact::default();
        let prompt_fields_changed = changed.iter().any(|key| {
            matches!(
                key.as_str(),
                "character" | "user_name" | "runtime_rules" | "user_persona"
            )
        });
        if changed.contains("mind") || changed.contains("ai") || prompt_fields_changed {
            impact.runtime_reload = true;
            self.abort_proactive_decision();
            self.abort_proactive_resolution();
        }
        if changed.contains("ai") {
            self.rebuild_tts_provider().await;
        }
        if changed.contains("rag") {
            // Rebuild the tool-RAG indexer from the fresh section so
            // selection options, embeddings, and the enabled flag take
            // effect without a restart.
            let embedder = self.session.memory.embedding_provider.clone();
            let concrete_store = self.concrete_store.clone();
            self.tool_rag = match embedder {
                Some(embedder) => match init_tool_rag(&self.config, &embedder, concrete_store) {
                    Ok(rag) => rag,
                    Err(e) => {
                        tracing::warn!(
                            component = "TurnActor",
                            error = %e,
                            "failed to rebuild tool RAG after settings apply"
                        );
                        None
                    }
                },
                None => None,
            };
            impact.runtime_reload = true;
        }
        if changed.contains("tools") {
            self.task_caps = self
                .config
                .get_section::<crate::task_config::ToolRuntimeConfig>()
                .unwrap_or_default();
            impact.runtime_reload = true;
        }
        if changed.contains("store") {
            impact.runtime_reload = true;
            let prev_store = prev
                .get_section::<ene_store::StoreConfig>()
                .unwrap_or_default();
            let next_store = request
                .config
                .get_section::<ene_store::StoreConfig>()
                .unwrap_or_default();
            // The SQLite connection is bound at bootstrap; flipping the
            // store on/off or switching between file and in-memory cannot be
            // re-bound live.
            if prev_store.enabled != next_store.enabled
                || prev_store.in_memory != next_store.in_memory
            {
                impact.app_restart = true;
            }
        }
        if changed.contains("plugins") {
            let prev_plugins = prev
                .get_section::<ene_plugin_host::PluginConfig>()
                .unwrap_or_default();
            let next_plugins = request
                .config
                .get_section::<ene_plugin_host::PluginConfig>()
                .unwrap_or_default();
            // MCP servers connect only at host start, so any change (add,
            // remove, transport edit, enable toggle) needs the same full
            // reconfigure as an enable-set change.
            if plugin_enable_set_changed(&prev_plugins, &next_plugins)
                || prev_plugins.mcp_servers != next_plugins.mcp_servers
            {
                impact.plugin_restart = true;
                self.spawn_reconfigure_plugin_host();
            } else {
                let updates = plugin_config_blob_updates(&prev_plugins, &next_plugins);
                if !updates.is_empty() {
                    impact.runtime_reload = true;
                    self.spawn_push_plugin_configs(updates);
                }
            }
        }

        self.sync_shared_config();
        self.settings_revision = self.settings_revision.saturating_add(1);
        Ok(crate::settings::SettingsApplyResult {
            revision: request.revision,
            current_revision: self.settings_revision,
            conflicted: false,
            applied_sections: applied,
            impact,
            errors,
        })
    }

    /// Resolves a pending permission request, including schedule-run
    /// confirmations that ride the same `PermissionRequired` path.
    async fn resolve_permission(&mut self, request_id: RequestId, decision: PermissionDecision) {
        let mut guard = self.pending_permissions.lock().await;
        if let Some(tx) = guard.remove(&request_id) {
            // A oneshot `Sender<PermissionDecision>::send` error is `Copy`
            // (it's just the unsent value), so `drop()` would itself trip
            // `clippy::dropping_copy_types`; a dropped receiver just means
            // the caller stopped waiting.
            #[expect(
                clippy::let_underscore_must_use,
                reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
            )]
            let _ = tx.send(decision);
        }
        drop(guard);
        if let Some(pending) = self.pending_schedule_confirmations.remove(&request_id) {
            pending.timeout.cancel();
            match decision {
                PermissionDecision::AllowOnce | PermissionDecision::AllowSession => {
                    self.begin_approved_scheduled_run(pending).await;
                }
                PermissionDecision::Deny => {
                    self.finish_scheduled_run(
                        pending.schedule_id,
                        pending.run_id,
                        ScheduleRunStatus::Denied,
                        None,
                    )
                    .await;
                }
            }
        }
    }

    /// Fetches settings snapshots for every configured plugin.
    async fn plugin_snapshots(&self) -> Vec<ene_plugin_host::PluginSettingsSnapshot> {
        let config = self.config.clone();
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager.settings_snapshots(&config).await,
            None => Vec::new(),
        }
    }

    /// Host-side artifact snapshot for the Engines page (empty when the
    /// artifact system is not configured).
    async fn artifact_snapshot(&self) -> Vec<ene_plugin_host::ArtifactSnapshot> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager.artifact_snapshot().await,
            None => Vec::new(),
        }
    }

    /// Installs or updates an artifact, then pushes re-injected config to
    /// live plugins.
    async fn install_artifact(
        &self,
        artifact_id: &str,
        version: Option<&str>,
    ) -> Result<ene_plugin_host::InstalledArtifactView, String> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager.install_artifact(artifact_id, version).await,
            None => Err("plugin host is not running".to_string()),
        }
    }

    /// Rolls an artifact back one generation.
    async fn rollback_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<ene_plugin_host::InstalledArtifactView, String> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager.rollback_artifact(artifact_id).await,
            None => Err("plugin host is not running".to_string()),
        }
    }

    /// Force-refreshes the signed catalog.
    async fn refresh_catalog(&self) -> Result<u64, String> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager.refresh_catalog().await,
            None => Err("plugin host is not running".to_string()),
        }
    }

    /// Fetches dynamic config options from one plugin (empty when the plugin
    /// is not connected or does not advertise `ListConfigOptions`).
    async fn list_plugin_config_options(
        &self,
        plugin: &str,
        path: &str,
    ) -> Result<Vec<ene_plugin_proto::ConfigOption>, EneRuntimeError> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager
                .list_config_options(plugin, path)
                .await
                .map_err(Into::into),
            None => Ok(Vec::new()),
        }
    }

    /// Validates a plugin config value through the plugin's own validator.
    async fn validate_plugin_config(
        &self,
        plugin: &str,
        value: &serde_json::Value,
    ) -> Result<Vec<ene_plugin_proto::ConfigFieldError>, EneRuntimeError> {
        let mut host = self.plugin_host.lock().await;
        match host.as_mut() {
            Some(manager) => manager
                .validate_config(plugin, value)
                .await
                .map_err(Into::into),
            None => Ok(Vec::new()),
        }
    }

    /// Start a new session when the user returns after a long idle period.
    fn maybe_split_session_on_timeout(&mut self) {
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let timeout = mind.session.session_timeout_minutes;
        if timeout == 0 {
            return;
        }
        let Some(last) = self.session.last_message_time() else {
            return;
        };
        let elapsed = chrono::Utc::now().signed_duration_since(last);
        let elapsed_minutes = elapsed.num_minutes();
        if elapsed_minutes < timeout as i64 {
            return;
        }
        let new_id = self.split_session();
        tracing::info!(
            component = "SessionSplit",
            elapsed_minutes,
            new_session_id = %new_id,
            "Session split due to inactivity timeout"
        );
    }

    async fn handle_manual_compression(&mut self) -> Result<CompressionResult, EneRuntimeError> {
        if self.session.history().is_empty() {
            return Err(EneRuntimeError::from(EneSessionError::SplitNotNeeded));
        }
        let Some(store) = self.session.memory.memory_store.clone() else {
            return Err(EneRuntimeError::from(EneSessionError::SplitNotNeeded));
        };
        let provider = self.create_provider().await?;
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let turns = self.mind_history_entries();
        let turn_end = self.session.current_turn_count() as i32;
        let turn_start = (turn_end - turns.len() as i32 / 2).max(0);
        let input = CompressionTaskInput {
            session_id: self.session.memory.session_id.to_string(),
            character_name: self.session.card_name().to_string(),
            user_name: self.config.user_name.clone(),
            turns,
            turn_start,
            turn_end,
            level: CompressionLevel::Scene,
            config: mind.resolved_context_config(),
            drop_leading: None,
        };
        let result = ene_mind::ContextManager::execute_manual(store, provider, input).await?;
        if compression_has_usable_summary(&result) {
            self.trim_history_after_compression();
        }
        Ok(result)
    }

    /// Evaluate window-pressure compression and spawn it when warranted.
    ///
    /// Resolves the chat provider only after
    /// [`ene_mind::ContextManager::should_trigger_window`] says compression
    /// would actually start, so a healthy-provider probe does not block the
    /// actor mailbox on every turn.
    async fn check_and_trigger_compression(&mut self) {
        let Ok(mem_config) = self.config.get_section::<ene_store::StoreConfig>() else {
            return;
        };
        if !mem_config.enabled {
            return;
        }
        let Ok(mind) = self.config.get_section::<ene_mind::MindConfig>() else {
            return;
        };
        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let turn_count = self.session.current_turn_count();
        let history = self.session.history().to_vec();
        if !self
            .context
            .should_trigger_window(&mind.context, turn_count, &history)
        {
            return;
        }
        let Ok(provider) = self.create_provider().await else {
            return;
        };
        self.context.check_and_trigger(
            &mind.resolved_context_config(),
            turn_count,
            &history,
            self.session.memory.session_id.as_str(),
            self.session.card_name(),
            &self.config.user_name,
            store,
            provider,
        );
    }

    async fn apply_pending_compression(&mut self) {
        if let Some(result) = self.context.poll_pending() {
            match result {
                Ok(compression) if compression_has_usable_summary(&compression) => {
                    tracing::info!(
                        component = "ContextCompression",
                        session_id = %compression.session_id,
                        span_id = compression.span_id,
                        level = ?compression.level,
                        "Rolling compression completed"
                    );
                    // Retroactive topic-boundary compression drops the
                    // pre-boundary prefix (keeping the boundary turn onward);
                    // window-pressure/manual compression trims to the
                    // configured recent window.
                    match compression.drop_leading {
                        Some(drop) => {
                            let keep = self.session.history().len().saturating_sub(drop);
                            self.trim_history_to_keep(keep);
                        }
                        None => self.trim_history_after_compression(),
                    }
                    self.spawn_chapter_rollup_if_needed().await;
                }
                Ok(compression) => {
                    tracing::warn!(
                        component = "ContextCompression",
                        session_id = %compression.session_id,
                        span_id = compression.span_id,
                        "Rolling compression finished without a usable summary; history preserved"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        component = "ContextCompression",
                        error = %error,
                        "Rolling compression failed"
                    );
                }
            }
        }
    }

    fn trim_history_after_compression(&mut self) {
        let recent_cap = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .map_or(16, |c| c.context.recent_turns.saturating_mul(2).max(2));
        self.trim_history_to_keep(recent_cap);
    }

    /// Trim the in-memory history down to the most recent `keep` messages.
    ///
    /// Used after a compression span is persisted: the compressed messages are
    /// dropped from the ring (the summary is served from the DB as the scene
    /// section), leaving the retained tail. `keep` is the recent window for
    /// window-pressure compression, or the boundary turn onward for
    /// retroactive topic-boundary compression.
    fn trim_history_to_keep(&mut self, keep: usize) {
        let history_len = self.session.history().len();
        if history_len > keep {
            self.session.trim_history_keep_last(keep);
        }
    }

    /// Spawn a retroactive compression for a detected topic boundary.
    ///
    /// Called from the actor's event loop once the stream task reports a
    /// boundary on the completed turn. The span before the boundary is
    /// summarized into a scene span; the resulting task is polled by
    /// [`Self::apply_pending_compression`] at the start of the next turn, which
    /// trims the history to the boundary turn onward. Never blocks the
    /// just-finished turn's response (Terminal was already emitted).
    async fn perform_retroactive_compression(&mut self, boundary_score: f32) {
        let Ok(mem_config) = self.config.get_section::<ene_store::StoreConfig>() else {
            return;
        };
        if !mem_config.enabled {
            return;
        }
        let Ok(mind) = self.config.get_section::<ene_mind::MindConfig>() else {
            return;
        };
        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let turn_count = self.session.current_turn_count();
        let history = self.session.history().to_vec();
        if !self
            .context
            .should_trigger_retroactive(turn_count, &history)
        {
            return;
        }
        let Ok(provider) = self.create_provider().await else {
            return;
        };
        self.context.check_and_trigger_retroactive(
            &mind.resolved_context_config(),
            turn_count,
            &history,
            boundary_score,
            self.session.memory.session_id.as_str(),
            self.session.card_name(),
            &self.config.user_name,
            store,
            provider,
        );
    }

    async fn spawn_chapter_rollup_if_needed(&self) {
        let Ok(mind) = self.config.get_section::<ene_mind::MindConfig>() else {
            return;
        };
        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let Ok(provider) = self.create_provider().await else {
            return;
        };
        ene_mind::ContextManager::spawn_chapter_rollup(
            store,
            provider,
            self.session.memory.session_id.to_string(),
            self.session.card_name().to_string(),
            self.config.user_name.clone(),
            mind.resolved_context_config(),
        );
    }

    fn mind_history_entries(&self) -> Vec<MindHistoryEntry> {
        self.session.history().to_vec()
    }
}

/// Completes vision preparation from loaded handles: validates the local
/// model, checks vision capability, and renders prompts.
///
/// Shared by the fast path (handles already loaded, reply sent synchronously
/// in the actor) and the slow path (handles loaded in background, reply sent
/// from `bg_command_tasks`). The `cancel` token is set by the caller after
/// this function returns `Ok`.
fn finish_vision_prep(
    handles: &crate::proactive_llm::ProactiveLlmHandles,
    prompt_language: &str,
    app_label: &str,
    hints: &crate::vision::ScreenSummaryHints,
) -> Result<VisionPrepared, crate::public_api::PublicApiError> {
    use crate::public_api::PublicApiError;

    let Some(local) = handles.local().cloned() else {
        return Err(PublicApiError::Internal {
            message: format!(
                "local proactive model is not available (decision_backend={:?})",
                handles.decision_kind
            ),
        });
    };

    let prompts = ene_config::PromptLibrary::load(prompt_language);
    let system = prompts.proactive().screen_summary_system.trim().to_string();
    let user = prompts.proactive().render_screen_summary_user(
        app_label,
        hints.roi_composited,
        hints.code_window,
        hints.ocr_text.as_deref(),
    );
    Ok(VisionPrepared {
        local,
        system,
        user,
        cancel: CancellationToken::new(), // Replaced by caller.
    })
}

/// Background plugin host reconfiguration.
///
/// Performs the heavy I/O (host shutdown, DB IPC spawn, host start, health
/// bridge) off the actor loop. Updates the shared `plugin_host`,
/// `health_bridge_handle`, and `host_service_handle` mutexes directly, then
/// sends [`EneCommand::PluginHostReconfigured`] back through the mailbox so
/// the actor can update its own `registry` and `plugin_tool_registries`
/// fields.
async fn reconfigure_plugin_host_bg(
    config: EneConfig,
    memory_store: Option<Arc<ene_store::MemoryStore>>,
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
    cmd_tx: mpsc::UnboundedSender<EneCommand>,
) {
    // Stop the previous host (and its health bridge) first. Provider
    // creation is served by whichever manager the shared slot holds, so the
    // old host's factories vanish with the swap below — no global
    // re-registration or stale-kind eviction is needed.
    {
        let mut guard = plugin_host.lock().await;
        if let Some(mut host) = guard.take() {
            host.shutdown().await;
        }
        drop(guard);

        let mut bridge = health_bridge_handle.lock().await;
        if let Some(handle) = bridge.take() {
            handle.abort();
        }
    }

    // Abort the previous host-service accept loop before rebinding the shared
    // socket path, or the stale listener keeps serving an unlinked socket
    // (unix) and the Windows rebind fails while the old pipe instance lives.
    if let Some(handle) = host_service_handle.lock().await.take() {
        handle.abort();
    }

    // The capability mediator resolves providers through the live host, so
    // it is wired before the host starts; calls landing during startup fail
    // with a typed "host is not running" error and are retryable.
    let mediator: Arc<dyn ene_plugin_proto::CapabilityServiceHandler> = Arc::new(
        ene_plugin_host::CapabilityMediator::new(Arc::clone(&plugin_host)),
    );
    let db_tokens = match spawn_db_ipc_servers(
        &config,
        memory_store.as_ref(),
        Some(Arc::clone(&mediator)),
        &cmd_tx,
    ) {
        Ok((tokens, new_handle)) => {
            *host_service_handle.lock().await = new_handle;
            tokens
        }
        Err(e) => {
            *host_service_handle.lock().await = None;
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Failed to spawn host service during plugin reconfiguration; \
                 continuing without plugin DB access"
            );
            HashMap::new()
        }
    };

    let mut new_host = match ene_plugin_host::PluginHostManager::start(&config, db_tokens).await {
        Ok(host) => Some(host),
        Err(e) => {
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Plugin host failed to restart after Features update; \
                 continuing without plugins"
            );
            None
        }
    };

    let mut bridge_handle: Option<tokio::task::JoinHandle<()>> = None;
    if let Some(host) = new_host.as_mut()
        && let Some(mut health_rx) = host.take_health_receiver()
    {
        let diag_tx = diag_tx.clone();
        let cmd_tx = cmd_tx.clone();
        let llm_factories_by_plugin = host.llm_factories_by_plugin();
        let embedding_factories_by_plugin = host.embedding_factories_by_plugin();
        let tts_factories_by_plugin = host.tts_factories_by_plugin();
        let stt_factories_by_plugin = host.stt_factories_by_plugin();
        let vad_factories_by_plugin = host.vad_factories_by_plugin();
        bridge_handle = Some(tokio::spawn(async move {
            while let Some(event) = health_rx.recv().await {
                if let PluginHealthEvent::Disabled { plugin, .. } = &event {
                    drop(
                        cmd_tx.send(EneCommand::PluginProviderDisabled {
                            plugin: plugin.clone(),
                            factories: ene_plugin_host::PluginFactoryHandles {
                                llm: llm_factories_by_plugin
                                    .get(plugin)
                                    .cloned()
                                    .unwrap_or_default(),
                                embedding: embedding_factories_by_plugin
                                    .get(plugin)
                                    .cloned()
                                    .unwrap_or_default(),
                                tts: tts_factories_by_plugin
                                    .get(plugin)
                                    .cloned()
                                    .unwrap_or_default(),
                                stt: stt_factories_by_plugin
                                    .get(plugin)
                                    .cloned()
                                    .unwrap_or_default(),
                                vad: vad_factories_by_plugin
                                    .get(plugin)
                                    .cloned()
                                    .unwrap_or_default(),
                            },
                        }),
                    );
                }
                emit_diag(&diag_tx, plugin_health_event_to_diag(event));
            }
        }));
    }

    let registries = new_host
        .as_ref()
        .map_or_else(Vec::new, |h| h.tool_registries().to_vec());
    let registry_count = registries.len();

    // Publish the new host and bridge handle to the shared slots *before*
    // notifying the actor: `PluginHostReconfigured` rebuilds the live TTS
    // provider through the slot, so it must already observe the new host.
    *plugin_host.lock().await = new_host;
    *health_bridge_handle.lock().await = bridge_handle;

    match CompositeToolRegistry::try_new(registries.clone()) {
        Ok(composite) => {
            tracing::info!(
                component = "TurnActor",
                tool_registries = registry_count,
                "Plugin host reconfigured and tool registry rebuilt after Features update"
            );
            drop(cmd_tx.send(EneCommand::PluginHostReconfigured {
                registry: Arc::new(composite),
                plugin_tool_registries: registries,
            }));
        }
        Err(e) => {
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Failed to rebuild tool registry after plugin reconfiguration"
            );
            // The registries were already refreshed above; only the actor's
            // live TTS provider still needs a rebuild, and without a tool
            // registry there is no `PluginHostReconfigured` to carry it.
            drop(cmd_tx.send(EneCommand::RebuildTtsProvider));
        }
    }
}

/// Runs a future to completion, catching any panic and surfacing it as a
/// [`DiagnosticEvent::ActorPanic`] instead of unwinding the caller.
///
/// Returns `Ok(output)` on normal completion, or `Err(message)` when the
/// future panicked. This keeps the actor command loop (and any other
/// supervisor site) alive across a panicking unit of work.
pub(super) async fn isolate_panic<F, T>(
    diag_tx: &broadcast::Sender<DiagnosticEvent>,
    component: &str,
    fut: F,
) -> Result<T, String>
where
    F: std::future::Future<Output = T>,
{
    use futures::FutureExt;
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(output) => Ok(output),
        Err(payload) => {
            let message = crate::diagnostics::panic_message(&payload);
            tracing::error!(
                component = "ActorSupervisor",
                component_name = %component,
                error = %message,
                "task panicked; contained by supervisor"
            );
            drop(diag_tx.send(DiagnosticEvent::ActorPanic {
                component: component.to_string(),
                message: message.clone(),
            }));
            Err(message)
        }
    }
}

/// Drains finished tasks from a `JoinSet`, logging any that panicked.
///
/// Keeps the actor's background task sets from growing without bound while
/// surfacing panics through structured diagnostics.
/// A `tokio::task::JoinHandle` that aborts its task when dropped.
///
/// A bare `JoinHandle` is "detach on drop": dropping it (for example because
/// the wrapper task that owned it was aborted by `JoinSet::abort_all()` on
/// shutdown) stops *supervising* the task but does not cancel it. Wrapping
/// the handle in this guard makes the abort propagate: dropping the
/// guard calls [`tokio::task::JoinHandle::abort`], so aborting (or dropping)
/// the wrapper aborts the underlying worker. Aborting a task that already
/// completed is a no-op, so wrapping a normally-finishing worker is harmless.
///
/// The guard must be constructed *outside* the wrapper future so it is part
/// of the future's captured state: if the wrapper is aborted before its first
/// poll, its future is dropped without ever running the body, and only
/// captured state is dropped. A guard created inside the body would never be
/// constructed in that case, leaving the inner handle merely detached.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn reap_join_set(
    set: &mut tokio::task::JoinSet<()>,
    component: &str,
    message: &str,
    diag_tx: &broadcast::Sender<DiagnosticEvent>,
) {
    while let Some(joined) = set.try_join_next() {
        if let Err(e) = joined {
            tracing::error!(component = %component, error = %e, "{message}");
            if e.is_panic() {
                drop(diag_tx.send(DiagnosticEvent::ActorPanic {
                    component: component.to_string(),
                    message: e.to_string(),
                }));
            }
        }
    }
}

/// Consumes a deferred memory-writer outcome and emits its side effects.
///
/// Used by supervised consumers in `memory_writer_tasks` (including the
/// `cap == 0` short-lived reject+consume path) so every non-hard-dropped
/// outcome produces identical events:
///
/// - [`ene_mind::MemoryWriteOutcome::Ok`] with pending candidates emits
///   [`LifecycleEvent::PendingCandidateAvailable`] on the lifecycle bus.
/// - [`ene_mind::MemoryWriteOutcome::Failed`] emits
///   [`DiagnosticEvent::MemoryWrite`],
///   with the current pending/permanent retry counts looked up from `store`
///   when one is available.
/// - A panicked writer task is logged and surfaced as a `failed`
///   [`DiagnosticEvent::MemoryWrite`].
async fn consume_memory_write_outcome(
    handle: tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>,
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
    store: Option<Arc<ene_store::MemoryStore>>,
) {
    match handle.await {
        Ok(ene_mind::MemoryWriteOutcome::Ok {
            deferred_candidates,
        }) => {
            if deferred_candidates > 0 {
                drop(
                    lifecycle_tx.send(LifecycleEvent::PendingCandidateAvailable {
                        count: deferred_candidates,
                    }),
                );
            }
        }
        Ok(ene_mind::MemoryWriteOutcome::Failed {
            message,
            pending_id,
            permanent,
            character_id,
        }) => {
            let (pending_count, permanent_count) = if let Some(store) = store.as_ref() {
                store
                    .count_pending_memory_writes(&character_id)
                    .await
                    .ok()
                    .map_or((None, None), |(p, f)| (Some(p as u64), Some(f as u64)))
            } else {
                (None, None)
            };
            let status = if permanent {
                "permanent"
            } else if pending_id.is_some() {
                "enqueued"
            } else {
                "failed"
            };
            drop(diag_tx.send(DiagnosticEvent::MemoryWrite {
                character_id,
                status: status.to_string(),
                message,
                pending_id,
                pending_count,
                permanent_count,
            }));
        }
        Err(e) => {
            tracing::error!(
                component = "MemoryWriter",
                error = %e,
                "Deferred memory-writer task panicked"
            );
            drop(diag_tx.send(DiagnosticEvent::MemoryWrite {
                character_id: String::new(),
                status: "failed".to_string(),
                message: format!("memory writer task panicked: {e}"),
                pending_id: None,
                pending_count: None,
                permanent_count: None,
            }));
        }
    }
}

/// Hard ceiling on supervised memory-writer outcome consumers (running +
/// waiting on the semaphore). Concurrent execution is still limited to
/// `memory_writer_cap` by the semaphore; this bound only stops the waiter
/// `JoinSet` from growing without limit under a burst.
fn memory_writer_hard_limit(cap: usize) -> usize {
    cap.saturating_mul(4)
}

/// Checks whether `set` has room for one more task under `cap` (Stage 8).
///
/// Returns `true` when the caller should proceed to `set.spawn(...)`.
/// Returns `false` when `set` is already at capacity: logs a warning and
/// emits [`DiagnosticEvent::TaskRejected`] so the rejection is observable
/// even for admission points with no reply channel of their own (mirrors
/// [`reap_join_set`]'s diag-reporting pattern). Callers with a reply
/// channel additionally send back [`crate::error::EneRuntimeError::Busy`]
/// so the rejection fails fast for the caller too, matching
/// `ene_infer::EngineError::Busy` / [`ene_ai::LlmProviderError::Busy`]
/// semantics.
///
/// ## Reaping before the capacity check
///
/// `JoinSet::len()` counts tasks that have *completed* but not yet been
/// joined (reaped). The run loop only reaps at the top of each iteration,
/// before it blocks in `select!`, so a task that finishes *during* the
/// block stays counted until the next iteration. A `CallTool` /
/// `SearchTools` command handled right after such a burst would then see a
/// stale, over-counted length and be rejected with a spurious
/// [`crate::error::EneRuntimeError::Busy`] that clears on the next
/// iteration. Reaping the set here — immediately before the capacity check
/// — removes finished tasks first so admission sees the true in-flight
/// count. `try_join_next` is non-blocking, so this is cheap, and it is
/// idempotent with the loop-top reap (a task is only ever joined once).
pub(super) fn admit_task(
    set: &mut tokio::task::JoinSet<()>,
    cap: usize,
    component: &str,
    detail: Option<String>,
    diag_tx: &broadcast::Sender<DiagnosticEvent>,
) -> bool {
    // Drop finished-but-unjoined tasks so `len()` reflects only tasks that
    // are actually still running. Without this, a burst that
    // completed during the actor's `select!` block would over-count and
    // spuriously reject the next admission.
    reap_join_set(set, component, "background task panicked", diag_tx);
    let queue_depth = set.len();
    if queue_depth < cap {
        return true;
    }
    tracing::warn!(
        component = %component,
        cap,
        queue_depth,
        detail = detail.as_deref().unwrap_or(""),
        "background task rejected: JoinSet at capacity (Stage 8)"
    );
    emit_diag(
        diag_tx,
        DiagnosticEvent::TaskRejected {
            component: component.to_string(),
            cap,
            detail,
        },
    );
    false
}

/// Polls a deferred (background) tool task until it reaches a terminal state.
///
/// Emits [`LifecycleEvent::ToolBackgroundCompleted`] on the lifecycle bus
/// (not the chat bus) when the task completes, fails, or is cancelled — it
/// fires asynchronously after the originating turn has already completed.
/// Runs as a background task in the actor's `deferred_tool_tasks`
/// `JoinSet`.
///
/// `max_polls` controls how many poll iterations (at 100ms each) before the task
/// is considered timed out. Configurable via `tools.deferred_max_polls`
/// (env `ENE_TOOLS__DEFERRED_MAX_POLLS`, default: 600 = 60s) — see
/// [`crate::task_config::ToolRuntimeConfig`].
async fn poll_deferred_task(
    registry: Arc<dyn ToolRegistry>,
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    task: DeferredToolTask,
    max_polls: u32,
) {
    use ene_plugin_proto::DeferredStatus;
    use std::time::Duration;

    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    for _ in 0..max_polls {
        let status = registry.poll_deferred(&task.tool_name, &task.task_id).await;
        match status {
            DeferredStatus::Pending => {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            DeferredStatus::Completed { result } => {
                drop(lifecycle_tx.send(LifecycleEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Completed { result },
                }));
                return;
            }
            DeferredStatus::Failed { error } => {
                drop(lifecycle_tx.send(LifecycleEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Failed { error },
                }));
                return;
            }
            DeferredStatus::Cancelled => {
                drop(lifecycle_tx.send(LifecycleEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Cancelled,
                }));
                return;
            }
            DeferredStatus::Unknown => {
                tracing::warn!(
                    component = "DeferredTool",
                    tool = %task.tool_name,
                    task_id = %task.task_id,
                    "Deferred task became unknown; stopping poll"
                );
                return;
            }
        }
    }

    tracing::warn!(
        component = "DeferredTool",
        tool = %task.tool_name,
        task_id = %task.task_id,
        "Deferred task polling timed out after {} polls",
        max_polls
    );
}

// ── Factory / init helpers ──

pub(super) async fn warmup_character_memories_ready(
    config: &EneConfig,
    session: &ConversationSession,
) -> Option<u64> {
    let mind = config
        .get_section::<ene_mind::MindConfig>()
        .unwrap_or_default();
    let card = session.character_card.as_ref()?;
    let store = session.memory.memory_store.as_ref()?;
    let embedder = session.memory.embedding_provider.as_ref()?;
    match ene_mind::character::sync_character_memories(
        store.as_ref(),
        embedder,
        session.card_name(),
        &config.user_name,
        card,
        &mind.character,
        None,
    )
    .await
    {
        Ok((report, hash)) => {
            tracing::info!(
                component = "Bootstrap",
                skipped = report.skipped,
                lorebook_inserted = report.lorebook_inserted,
                lorebook_updated = report.lorebook_updated,
                style_inserted = report.style_inserted,
                style_updated = report.style_updated,
                archived = report.archived,
                "Character memory warmup complete"
            );
            Some(hash)
        }
        Err(e) => {
            tracing::warn!(
                component = "Bootstrap",
                error = %e,
                "Character memory warmup failed; first turn will retry"
            );
            None
        }
    }
}

fn plugin_enable_set_changed(
    prev: &ene_plugin_host::PluginConfig,
    next: &ene_plugin_host::PluginConfig,
) -> bool {
    if prev.enabled != next.enabled {
        return true;
    }
    let mut keys: Vec<&String> = prev.list.keys().chain(next.list.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let prev_enable = prev.list.get(key).is_none_or(|e| e.enable);
        let next_enable = next.list.get(key).is_none_or(|e| e.enable);
        if prev_enable != next_enable {
            return true;
        }
    }
    false
}

/// Returns per-plugin delivered config/profiles blobs that changed between
/// `prev` and `next`, for plugins that remain enabled in both.
///
/// Used to hot-push `SetConfig` when the enable-set is unchanged. Compares
/// the nested `config` / `profiles` maps and legacy flat `extra` keys that
/// fold into the delivered blob.
fn plugin_config_blob_updates(
    prev: &ene_plugin_host::PluginConfig,
    next: &ene_plugin_host::PluginConfig,
) -> HashMap<String, (Option<serde_json::Value>, Option<serde_json::Value>)> {
    let mut updates = HashMap::new();
    let mut keys: Vec<&String> = prev.list.keys().chain(next.list.keys()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let prev_entry = prev.list.get(key);
        let next_entry = next.list.get(key);
        let prev_enable = prev_entry.is_none_or(|e| e.enable);
        let next_enable = next_entry.is_none_or(|e| e.enable);
        if !next_enable || prev_enable != next_enable {
            continue;
        }
        let Some(next_e) = next_entry else {
            continue;
        };
        let changed = prev_entry.is_none_or(|p| {
            p.config != next_e.config || p.profiles != next_e.profiles || p.extra != next_e.extra
        });
        if changed {
            updates.insert(
                key.clone(),
                (next_e.delivered_config(key), next_e.delivered_profiles()),
            );
        }
    }
    updates
}

/// Plugin DB auth tokens plus the host-service accept-loop handle.
///
/// The handle is `None` when no DB-capable plugin exists (no endpoint bound)
/// and must be aborted before re-binding on reconfiguration or shutdown.
pub(super) type DbIpcServers = (HashMap<String, String>, Option<tokio::task::JoinHandle<()>>);

/// Spawns the shared host-service acceptor with a `db` passenger for each
/// discovered tool plugin.
///
/// Returns a map of plugin name → auth token. The tokens are passed to
/// [`ene_plugin_host::PluginHostManager::start`] which hands them to the
/// tool binaries via `SandboxConfigData::db_auth_token`.
///
/// Token generation is driven by the **detected plugin set** (the binaries
/// actually discovered on disk) rather than by `plugins.list` config keys.
/// The host manager discovers plugins by scanning for `ene-plugin-{name}`
/// binaries and only consults config to skip explicitly-disabled names, so
/// keying tokens off config would orphan registrations (config key with no
/// matching binary) or starve plugins of tokens (binary with no config
/// entry). Mirroring the manager's discovery here keeps the two sets aligned.
///
/// The returned `JoinHandle` owns the accept loop; the caller must abort it
/// before re-spawning (reconfiguration) or on shutdown so the old listener
/// does not keep serving an unlinked socket (unix) or block the pipe rebind
/// (Windows).
pub(super) fn spawn_db_ipc_servers(
    config: &EneConfig,
    memory_store: Option<&Arc<ene_store::MemoryStore>>,
    capability_handler: Option<Arc<dyn ene_plugin_proto::CapabilityServiceHandler>>,
    cmd_tx: &mpsc::UnboundedSender<EneCommand>,
) -> Result<DbIpcServers, EneRuntimeError> {
    let mut db_tokens = HashMap::new();
    let Some(store) = memory_store else {
        return Ok((db_tokens, None));
    };

    #[cfg(any(unix, windows))]
    {
        let plugin_config = config
            .get_section::<ene_plugin_host::PluginConfig>()
            .unwrap_or_default();

        // When the plugin system is disabled the host manager spawns nothing,
        // so no host-service acceptor is needed.
        if !plugin_config.enabled {
            return Ok((db_tokens, None));
        }

        let db = store.connection().clone();

        let socket_dir = ene_config::paths::tool_socket_dir();
        std::fs::create_dir_all(&socket_dir).map_err(|e| {
            EneRuntimeError::Tool(PluginHostError::ExecutionFailed {
                message: format!("Failed to create socket dir: {e}"),
            })
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // 0o700 on the parent dir makes the bind→chmod window on the socket
            // file unreachable from other users (the socket exists with umask-based
            // perms for the few syscalls between bind and set_permissions).
            if let Err(e) =
                std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700))
            {
                return Err(EneRuntimeError::Tool(PluginHostError::ExecutionFailed {
                    message: format!("Failed to tighten tool socket dir permissions: {e}"),
                }));
            }
        }

        let mut db_plugins = HashMap::new();

        for name in discover_plugin_names() {
            // Skip plugins explicitly disabled in configuration, mirroring
            // `PluginHostManager::start`'s enable filter (a discovered binary
            // with no config entry is enabled by default).
            let entry = plugin_config.list.get(&name);
            if let Some(entry) = entry
                && !entry.enable
            {
                continue;
            }

            // Per-plugin DB storage quota. A discovered binary with no
            // config entry falls back to `PluginEntry::default()`'s quota so
            // the enforcement default applies uniformly; an explicit `null` in
            // config (`None`) disables the cap for that plugin.
            let quota_mb = match entry {
                Some(entry) => entry.db_quota_mb,
                None => ene_plugin_host::PluginEntry::default().db_quota_mb,
            };
            let quota_bytes = quota_mb.map(|mb| mb.saturating_mul(1024 * 1024));

            // Generate a 128-bit pre-shared token for this tool's
            // host-service `db` session. We use a 256-bit keystream from
            // blake3 (already a dep) keyed by the current nanosecond
            // timestamp + a monotonic counter. blake3 is a CSPRNG
            // and the counter guarantees uniqueness across calls in
            // the same nanosecond.
            let counter = DB_TOKEN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut hasher = blake3::Hasher::new();
            hasher.update(
                &chrono::Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or(0)
                    .to_le_bytes(),
            );
            hasher.update(&counter.to_le_bytes());
            let mut token_out = [0u8; 16];
            let mut reader = hasher.finalize_xof();
            use std::io::Read;
            drop(reader.read_exact(&mut token_out));
            let auth_token = format!("ene-db-{:x}", u128::from_le_bytes(token_out));

            db_tokens.insert(name.clone(), auth_token.clone());
            db_plugins.insert(
                auth_token,
                DbPluginRegistration {
                    tool_name: name.clone(),
                    prefix: format!("{name}_"),
                    quota_bytes,
                },
            );
        }

        // No DB plugins → no need to bind the shared endpoint.
        if db_plugins.is_empty() {
            return Ok((db_tokens, None));
        }

        let socket_path = host_service_socket_path();
        let mut server = HostServiceServer::new(db, socket_path, db_plugins);
        if let Some(handler) = capability_handler {
            server = server.with_capability_handler(handler);
        }
        if let Some(broker) = ene_plugin_host::BrokerHub::from_config(config) {
            let responder = crate::approval::ActorApprovalResponder::new(
                cmd_tx.clone(),
                std::time::Duration::from_millis(plugin_config.permission_prompt_timeout_ms),
            );
            server = server.with_broker_handler(
                broker.with_approval_responder(std::sync::Arc::new(responder)),
            );
        }

        let handle = tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::error!(error = %e, "Host service server error");
            }
        });

        Ok((db_tokens, Some(handle)))
    }

    #[cfg(not(any(unix, windows)))]
    {
        Ok((db_tokens, None))
    }
}

/// Discovers plugin binary names by scanning the builtin and user plugin
/// directories for executables following the `ene-plugin-{name}` convention.
///
/// This intentionally mirrors `ene_plugin_host::manager::discover_plugins`
/// (which is private to that crate) so that host-service token generation keys
/// off the exact same set of plugins the host manager will actually spawn.
/// Keeping the two discovery routines in lockstep is what prevents
/// config-key ↔ binary-name mismatches from orphaning registrations or
/// starving plugins of tokens.
#[cfg(any(unix, windows))]
fn discover_plugin_names() -> Vec<String> {
    let mut names = Vec::new();
    let exe_suffix = if cfg!(windows) { ".exe" } else { "" };

    for dir in [
        ene_config::builtin_plugins_dir(),
        ene_config::user_plugins_dir(),
    ] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || !is_plugin_executable(&path) {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let stem = file_name
                .strip_suffix(exe_suffix)
                .unwrap_or(file_name)
                .to_string();

            // Only the `ene-plugin-{name}` convention is accepted; a bare
            // `{name}` fallback is intentionally omitted to avoid spawning
            // unrelated build artifacts (see the manager's discovery docs).
            let Some(plugin_name) = stem.strip_prefix("ene-plugin-") else {
                continue;
            };
            let plugin_name = plugin_name.to_string();

            if !plugin_name.is_empty() && !names.contains(&plugin_name) {
                names.push(plugin_name);
            }
        }
    }

    names
}

/// Returns `true` when `path` has an executable permission bit set.
///
/// On Unix this checks the mode bits; on non-Unix targets every existing file
/// is considered executable (the `.exe` suffix already gates matching there).
#[cfg(any(unix, windows))]
fn is_plugin_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

pub(super) fn build_tool_registry(
    plugin_host: Option<&ene_plugin_host::PluginHostManager>,
) -> Result<Arc<dyn ToolRegistry>, EneRuntimeError> {
    let Some(host) = plugin_host else {
        return Ok(Arc::new(
            CompositeToolRegistry::try_new(vec![]).map_err(EneRuntimeError::Tool)?,
        ));
    };

    let registries = host.tool_registries().to_vec();
    CompositeToolRegistry::try_new(registries)
        .map(|composite| Arc::new(composite) as Arc<dyn ToolRegistry>)
        .map_err(EneRuntimeError::Tool)
}

/// Maps a [`PluginHealthEvent`] to a [`DiagnosticEvent::ToolHealth`]
/// with a stable English status contract.
fn plugin_health_event_to_diag(event: PluginHealthEvent) -> DiagnosticEvent {
    let (tool, status, detail) = match event {
        PluginHealthEvent::Unhealthy { plugin, reason } => {
            (plugin, "unhealthy", Some(format!("tool is {reason}")))
        }
        PluginHealthEvent::Restarting { plugin, attempt } => (
            plugin,
            "restarting",
            Some(format!("restart attempt {attempt}")),
        ),
        PluginHealthEvent::Restarted { plugin } => (plugin, "restarted", None),
        PluginHealthEvent::Recovered { plugin } => (plugin, "recovered", None),
        PluginHealthEvent::CircuitOpened {
            plugin,
            consecutive_failures,
        } => (
            plugin,
            "circuit_open",
            Some(format!("{consecutive_failures} consecutive failures")),
        ),
        PluginHealthEvent::CircuitClosed { plugin } => (plugin, "circuit_closed", None),
        PluginHealthEvent::Disabled { plugin, reason } => {
            // `status` stays the stable `"disabled"`; the detail explains why.
            let detail = match reason {
                DisabledReason::ChecksumMismatch => "binary checksum mismatch on restart",
                DisabledReason::RestartBudgetExhausted => "restart budget exhausted",
            };
            (plugin, "disabled", Some(detail.to_string()))
        }
        PluginHealthEvent::RequirementsUnmet {
            plugin,
            requirements,
        } => (
            plugin,
            "disabled",
            Some(format!(
                "unmet capability requirements: {}",
                requirements.join(", ")
            )),
        ),
    };
    DiagnosticEvent::ToolHealth {
        tool,
        status: status.to_string(),
        detail,
    }
}

pub(super) fn init_embedding(
    config: &EneConfig,
    provider_host: Arc<dyn ProviderHost>,
) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
    use ene_ai::ResolvedEmbedding;

    let ai_config = config
        .get_section::<ene_ai::AiConfig>()
        .map_err(|e| ene_ai::EmbeddingError::Init(format!("Failed to load AI config: {e}")))?;
    match ai_config
        .resolve_embedding()
        .map_err(|e| ene_ai::EmbeddingError::Init(e.to_string()))?
    {
        ResolvedEmbedding::Local(local) => {
            // Same bootstrap-ordering indirection as the cloud path: the
            // plugin host (which registers the "local" embedding factory)
            // starts after the embedder, because the host needs DB tokens
            // derived from the memory store, which needs the embedder's
            // dimensions. The proxy answers dimensions from config and only
            // touches the registry on first use.
            let dimensions = local.dimensions.ok_or_else(|| {
                ene_ai::EmbeddingError::Init(format!(
                    "local embedding model {:?} requires ai.local_models.{}.dimensions",
                    local.name, local.name
                ))
            })?;
            Ok(Arc::new(PluginEmbeddingProxy::new(
                ene_ai::LOCAL_PROVIDER.to_string(),
                local.name,
                dimensions,
                Arc::new(config.clone()),
                Arc::clone(&provider_host),
            )))
        }
        ResolvedEmbedding::Cloud {
            kind,
            model,
            dimensions,
            ..
        } => Ok(Arc::new(PluginEmbeddingProxy::new(
            kind,
            model,
            dimensions,
            Arc::new(config.clone()),
            provider_host,
        ))),
    }
}

/// An embedding provider that resolves its backend lazily from the provider
/// host on first use.
///
/// Bootstrap ordering forces this indirection: the plugin host (which
/// serves plugin embedding factories) starts *after* the embedder is
/// created, because the host needs DB tokens that are derived from the
/// memory store, which in turn needs the embedder's dimensions. Model and
/// dimensions are config-derived, so the proxy answers those synchronously
/// and only touches the host when a real embedding is requested — by which
/// time the host has started. Resolution is per-call, so a plugin host
/// restart (with a swapped-in manager) is picked up automatically.
struct PluginEmbeddingProxy {
    kind: String,
    model: String,
    dimensions: usize,
    config: Arc<EneConfig>,
    provider_host: Arc<dyn ProviderHost>,
}

impl PluginEmbeddingProxy {
    fn new(
        kind: String,
        model: String,
        dimensions: usize,
        config: Arc<EneConfig>,
        provider_host: Arc<dyn ProviderHost>,
    ) -> Self {
        Self {
            kind,
            model,
            dimensions,
            config,
            provider_host,
        }
    }

    async fn resolve(&self) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
        self.provider_host
            .create_embedding_provider(&self.kind, &self.config)
            .await
    }
}

#[async_trait::async_trait]
impl ene_ai::EmbeddingProvider for PluginEmbeddingProxy {
    async fn embed_batch(
        &self,
        items: &[(&str, ene_ai::EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
        self.resolve().await?.embed_batch(items).await
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

pub(super) async fn init_memory_store(
    config: &EneConfig,
    embedder: &dyn ene_ai::EmbeddingProvider,
) -> Result<Arc<ene_store::MemoryStore>, String> {
    let store_config = config
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default();
    let db_path = store_config.resolve_memory_db_path(&config.character);

    if db_path != std::path::Path::new(":memory:")
        && let Some(parent) = db_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create memory DB directory: {e}"))?;
    }

    let dims = embedder.dimensions();
    let options = ene_store::OpenOptions::from(&store_config);
    let store = ene_store::MemoryStore::open_with_options(&db_path, dims, &options)
        .await
        .map_err(|e| format!("Failed to open memory store: {e}"))?;

    Ok(Arc::new(store))
}

/// Builds the `ToolRag` pipeline from the current config, embedder, and session state.
///
/// Returns `None` when the pipeline is disabled. Invalid `forced` tool
/// names fail `EneHandle::open`.
pub(super) fn init_tool_rag(
    config: &EneConfig,
    embedder: &Arc<dyn ene_ai::EmbeddingProvider>,
    concrete_store: Option<Arc<ene_store::MemoryStore>>,
) -> Result<Option<Arc<ToolRag>>, EneRuntimeError> {
    let rag_config = config.get_section::<ToolRagConfig>()?;

    if !rag_config.enabled {
        return Ok(None);
    }

    // The tool RAG persists embeddings through the `EmbeddingStorePort`
    // abstraction so `ene-rag` never depends on `ene-store`.
    let store: Option<Arc<dyn ene_core::EmbeddingStorePort>> = concrete_store
        .clone()
        .map(|s| s as Arc<dyn ene_core::EmbeddingStorePort>);
    let opts = ToolRagOptions::from_config(rag_config)?;
    let mut rag = ToolRag::new(embedder.clone(), store, opts);
    // Recent tool-failure feedback: let selection down-weight tools that
    // recently failed, read through a port so `ene-rag` stays store-agnostic.
    if let Some(store) = concrete_store {
        rag = rag.with_failure_signals(store as Arc<dyn ene_core::ToolFailureSignalPort>);
    }
    Ok(Some(Arc::new(rag)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_health_parts(event: DiagnosticEvent) -> (String, String, Option<String>) {
        match event {
            DiagnosticEvent::ToolHealth {
                tool,
                status,
                detail,
            } => (tool, status, detail),
            other => panic!("expected ToolHealth, got {other:?}"),
        }
    }

    /// The `Disabled` detail string is derived from the structured
    /// [`DisabledReason`] via an exhaustive match: both variants must
    /// map to their stable, human-readable detail while the `status` contract
    /// stays `"disabled"`.
    #[test]
    fn disabled_reason_maps_to_detail_string() {
        let (tool, status, detail) =
            tool_health_parts(plugin_health_event_to_diag(PluginHealthEvent::Disabled {
                plugin: "fs".to_string(),
                reason: DisabledReason::ChecksumMismatch,
            }));
        assert_eq!(tool, "fs");
        assert_eq!(status, "disabled");
        assert_eq!(
            detail.as_deref(),
            Some("binary checksum mismatch on restart")
        );

        let (tool, status, detail) =
            tool_health_parts(plugin_health_event_to_diag(PluginHealthEvent::Disabled {
                plugin: "web".to_string(),
                reason: DisabledReason::RestartBudgetExhausted,
            }));
        assert_eq!(tool, "web");
        assert_eq!(status, "disabled");
        assert_eq!(detail.as_deref(), Some("restart budget exhausted"));
    }

    /// The startup gate emits [`PluginHealthEvent::RequirementsUnmet`] for
    /// plugins whose hard capability requirements have no provider; the
    /// bridge maps it to the same `"disabled"` status with the unmet
    /// requirements named in the detail. Kept in lockstep with the
    /// bootstrap-time mapper's test.
    #[test]
    fn requirements_unmet_maps_to_disabled_detail() {
        let (tool, status, detail) = tool_health_parts(plugin_health_event_to_diag(
            PluginHealthEvent::RequirementsUnmet {
                plugin: "consumer".to_string(),
                requirements: vec!["gguf-runner@^1".to_string()],
            },
        ));
        assert_eq!(tool, "consumer");
        assert_eq!(status, "disabled");
        assert_eq!(
            detail.as_deref(),
            Some("unmet capability requirements: gguf-runner@^1")
        );
    }

    /// Regression test: a worker routed through `aux_task_tx` is
    /// actually aborted on shutdown, not merely detached.
    ///
    /// The stand-in for the TTS worker never exits on its own — it loops
    /// forever ticking a counter — so only the shutdown drain + `abort_all`
    /// (the [`AbortOnDrop`] guard inside the wrapper task) can stop it. The
    /// counter must stop advancing after [`EneCommand::Shutdown`], and the
    /// actor's cancel token must be cancelled so cooperative workers (the
    /// real TTS pipeline watches it) see the shutdown too.
    #[tokio::test]
    async fn shutdown_aborts_aux_worker_and_cancels_token() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use std::sync::atomic::AtomicUsize;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);

        let ticks = Arc::new(AtomicUsize::new(0));
        let ticks_in_worker = Arc::clone(&ticks);
        let worker = tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                ticks_in_worker.fetch_add(1, Ordering::SeqCst);
            }
        });
        // Route the worker through the actor's aux channel, exactly as the
        // TTS pipeline does with `aux_task_tx`.
        drop(actor.aux_task_tx.send(worker));

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(ticks.load(Ordering::SeqCst) > 0, "worker never started");

        assert!(
            !actor.handle_command(EneCommand::Shutdown).await,
            "Shutdown must signal the run loop to exit"
        );
        assert!(
            actor.cancel_token.is_cancelled(),
            "Shutdown must cancel the turn's cancel token"
        );

        // The abort is delivered at the worker's next yield (its 1 ms sleep),
        // so let it land before sampling, then verify the counter is stable.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let after_shutdown = ticks.load(Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let later = ticks.load(Ordering::SeqCst);
        assert_eq!(
            after_shutdown, later,
            "aux worker kept ticking after shutdown: the abort did not propagate"
        );
    }

    /// Regression test: `EneCommand::Shutdown` must release
    /// the single-flight [`TurnGate`].
    ///
    /// A `run()` racing the actor's teardown could otherwise see a gate
    /// the dying actor still held and be reported `RunError::Busy` instead of
    /// `RunError::ActorDead`. Releasing the gate on shutdown (combined with
    /// `EneHandle::run` checking the closed channel first) ensures a dead
    /// actor is reported as dead, not busy.
    #[tokio::test]
    async fn shutdown_releases_single_flight_gate() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor_with_gate};

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx, _lifecycle_rx, gate) =
            build_bare_actor_with_gate(registry, &task_caps);

        // Simulate a turn having claimed the gate (as `EneHandle::run` does
        // via `try_begin`) and not yet released it.
        let turn = TurnId::new();
        assert!(gate.try_begin(&turn), "gate must be free before the turn");
        assert!(gate.active.lock().is_some(), "gate is held mid-turn");

        assert!(
            !actor.handle_command(EneCommand::Shutdown).await,
            "Shutdown must signal the run loop to exit"
        );
        assert!(
            gate.active.lock().is_none(),
            "Shutdown must release the single-flight gate so a racing run() \
             reports ActorDead, not a stale Busy"
        );
    }

    /// The mailbox-free shared session state must be published the
    /// moment a session split resets the session: the shared session
    /// id and started-at change, and the shared turn count resets to zero.
    /// Drives the same `split_session` helper the inactivity-timeout path
    /// (`maybe_split_session_on_timeout`) calls once its elapsed check fires,
    /// so a regression in the split → publish wiring is caught here.
    #[tokio::test]
    async fn timeout_split_publishes_shared_session_state() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor_with_session_and_gate};

        let mut session = ConversationSession::new();
        // Start the turn count at 1 so the post-split reset assertion below
        // compares 1 -> 0 instead of the vacuous 0 == 0.
        session.record_user_input();
        let config = EneConfig::default();
        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx, _lifecycle_rx, _gate, shared) =
            build_bare_actor_with_session_and_gate(registry, &task_caps, session, config);

        let old_id = shared.session.read().session_id.clone();
        let old_started_at = shared.session.read().session_started_at;
        assert_eq!(
            shared.session.read().turn_count,
            1,
            "the injected session's turn count is mirrored into the shared state"
        );

        actor.split_session();

        let after = shared.session.read().clone();
        assert_ne!(
            after.session_id, old_id,
            "a session split must publish the new session id to the shared state"
        );
        assert_ne!(
            after.session_started_at, old_started_at,
            "a session split must publish the new session start time"
        );
        assert_eq!(
            after.turn_count, 0,
            "a session split resets the shared turn count from 1 to zero"
        );
    }

    fn quiet_hours_actor(
        actor: &mut TurnActor,
        policy: ene_mind::QuietHoursPolicy,
        suppress: ene_mind::QuietHoursSuppressConfig,
        fixed: chrono::DateTime<chrono::Utc>,
    ) {
        let mut mind = ene_mind::MindConfig::default();
        mind.proactive.enabled = true;
        mind.proactive.min_idle_seconds = 0;
        mind.proactive.cooldown_seconds = 0;
        mind.proactive.quiet_hours = ene_mind::QuietHoursConfig {
            enabled: true,
            timezone: "UTC".into(),
            days: ene_mind::QuietHoursDaysConfig {
                monday: true,
                ..ene_mind::QuietHoursDaysConfig::default()
            },
            start: ene_mind::QuietHoursTimeConfig {
                hour: 22,
                minute: 0,
            },
            end: ene_mind::QuietHoursTimeConfig {
                hour: 23,
                minute: 0,
            },
            suppress,
            policy,
        };
        actor
            .config
            .set_section(&mind)
            .expect("set mind config on test actor");
        actor.quiet_hours_clock = Arc::new(move || fixed);
    }

    #[tokio::test]
    async fn quiet_hours_suppress_decisions_queues_and_skips_spawn() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Queue,
            ene_mind::QuietHoursSuppressConfig::default(),
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        // Give the warrant gates something to pass: a session with history
        // and an observation with activity, so the tick is a real suppressed
        // opportunity rather than a NoSources rejection.
        actor.session.record_user_input();
        actor.session.add_user_message("hi");
        actor.proactive.observation = ene_mind::ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ene_mind::ActivitySnapshot::default()),
            screen_summary: None,
            screen_summary_status: ene_mind::ScreenSummaryStatus::default(),
        };

        actor.maybe_spawn_proactive_decision().await;

        assert!(
            actor.proactive_decision_rx.is_none(),
            "no decision task may spawn during quiet hours"
        );
        assert_eq!(actor.proactive.quiet_hours_queue.len(), 1);
        assert_eq!(
            actor
                .proactive
                .quiet_hours_queue
                .front()
                .map(|e| e.local_date.as_str()),
            Some("2026-08-03")
        );
    }

    #[tokio::test]
    async fn quiet_hours_catch_up_keeps_queue_when_generation_cannot_start() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Queue,
            ene_mind::QuietHoursSuppressConfig::default(),
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        actor.session.record_user_input();
        actor.session.add_user_message("hi");
        actor.proactive.observation = ene_mind::ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ene_mind::ActivitySnapshot::default()),
            screen_summary: None,
            screen_summary_status: ene_mind::ScreenSummaryStatus::default(),
        };
        actor.maybe_spawn_proactive_decision().await;
        assert_eq!(actor.proactive.quiet_hours_queue.len(), 1);

        actor.quiet_hours_clock = Arc::new(|| {
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 23, 30, 0)
                .single()
                .expect("valid utc instant")
        });
        actor.maybe_spawn_proactive_decision().await;

        assert_eq!(
            actor.proactive.quiet_hours_queue.len(),
            1,
            "a failed generation start (no provider in the bare actor) must keep the queued moment"
        );
        assert!(
            actor.proactive.last_decision.is_none(),
            "start_stream failure must clear the pending decision"
        );
    }

    #[tokio::test]
    async fn engaged_world_state_skips_the_quiet_hours_queue() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;
        use std::time::{SystemTime, UNIX_EPOCH};

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Queue,
            ene_mind::QuietHoursSuppressConfig::default(),
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        let mut mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");
        mind.proactive.world_state.enabled = true;
        actor
            .config
            .set_section(&mind)
            .expect("set mind config on test actor");
        actor.session.record_user_input();
        actor.session.add_user_message("hi");
        let captured_at_unix_ms: u64 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_millis()
            .try_into()
            .expect("current Unix timestamp fits in u64");

        // Three observations with a fresh window switch: the trend is
        // engaged, so the quiet-hours tick is not an utterance opportunity.
        for i in 0..3 {
            assert!(
                actor
                    .handle_command(EneCommand::UpdateProactiveObservation {
                        observation: ene_mind::ProactiveObservation {
                            captured_at_unix_ms: captured_at_unix_ms + i,
                            activity: Some(ene_mind::ActivitySnapshot {
                                idle_seconds: None,
                                active_window_label: "Editor".into(),
                                recent_change: "switched".into(),
                            }),
                            screen_summary: None,
                            screen_summary_status: ene_mind::ScreenSummaryStatus::Disabled,
                        },
                    })
                    .await
            );
        }

        actor.maybe_spawn_proactive_decision().await;

        assert!(
            actor.proactive.quiet_hours_queue.is_empty(),
            "an engaged user inside quiet hours must not queue a catch-up moment"
        );
        assert!(actor.proactive_decision_rx.is_none());
    }

    #[tokio::test]
    async fn manual_pause_clears_the_catch_up_queue() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Queue,
            ene_mind::QuietHoursSuppressConfig::default(),
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        actor.session.record_user_input();
        actor.session.add_user_message("hi");
        actor.proactive.observation = ene_mind::ProactiveObservation {
            captured_at_unix_ms: 1,
            activity: Some(ene_mind::ActivitySnapshot::default()),
            screen_summary: None,
            screen_summary_status: ene_mind::ScreenSummaryStatus::default(),
        };
        actor.maybe_spawn_proactive_decision().await;
        assert_eq!(actor.proactive.quiet_hours_queue.len(), 1);

        // Pause wins over quiet hours: no queue entry, and the pending queue
        // is dropped.
        let mut mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");
        mind.proactive.paused = true;
        actor
            .config
            .set_section(&mind)
            .expect("set mind config on test actor");
        actor.quiet_hours_clock = Arc::new(|| {
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 23, 30, 0)
                .single()
                .expect("valid utc instant")
        });
        actor.maybe_spawn_proactive_decision().await;

        assert!(actor.proactive.quiet_hours_queue.is_empty());
        assert!(actor.proactive_decision_rx.is_none());
    }

    #[tokio::test]
    async fn observation_updates_feed_the_world_state_ring() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use std::time::{SystemTime, UNIX_EPOCH};

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        let captured_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_millis()
            .try_into()
            .expect("current Unix timestamp fits in u64");

        let observation = ene_mind::ProactiveObservation {
            captured_at_unix_ms,
            activity: Some(ene_mind::ActivitySnapshot {
                idle_seconds: Some(30),
                active_window_label: "Code".into(),
                recent_change: "focused Code".into(),
            }),
            screen_summary: None,
            screen_summary_status: ene_mind::ScreenSummaryStatus::Disabled,
        };
        assert!(
            actor
                .handle_command(EneCommand::UpdateProactiveObservation {
                    observation: observation.clone(),
                })
                .await
        );
        assert_eq!(actor.proactive.world_state.len(), 1);
        let snap = actor
            .proactive
            .world_state
            .latest()
            .expect("latest world-state snapshot");
        assert_eq!(snap.captured_at_unix_ms, captured_at_unix_ms);
        assert_eq!(snap.idle_seconds, Some(30));
        assert_eq!(snap.active_window_label, "Code");
        assert_eq!(snap.recent_change, "focused Code");

        assert!(
            actor
                .handle_command(EneCommand::UpdateProactiveObservation { observation })
                .await
        );
        assert_eq!(actor.proactive.world_state.len(), 2);

        // A delayed observation may still arrive after the decision payload
        // would be rejected as stale; it must not reintroduce old activity
        // into the world-state summary.
        assert!(
            actor
                .handle_command(EneCommand::UpdateProactiveObservation {
                    observation: ene_mind::ProactiveObservation {
                        captured_at_unix_ms: 1,
                        activity: Some(ene_mind::ActivitySnapshot {
                            idle_seconds: Some(0),
                            active_window_label: "Stale editor".into(),
                            recent_change: "stale focus".into(),
                        }),
                        screen_summary: None,
                        screen_summary_status: ene_mind::ScreenSummaryStatus::Disabled,
                    },
                })
                .await
        );
        assert_eq!(actor.proactive.world_state.len(), 3);
        let stale = actor
            .proactive
            .world_state
            .latest()
            .expect("latest world-state snapshot");
        assert_eq!(stale.idle_seconds, None);
        assert!(stale.active_window_label.is_empty());
        assert!(stale.recent_change.is_empty());

        // A zero-timestamp observation (no host yet) must not pollute the
        // ring.
        assert!(
            actor
                .handle_command(EneCommand::UpdateProactiveObservation {
                    observation: ene_mind::ProactiveObservation::default(),
                })
                .await
        );
        assert_eq!(actor.proactive.world_state.len(), 3);

        // A session reset drops the world-state history with the rest of the
        // per-session proactive state.
        actor.proactive.reset_session();
        assert!(actor.proactive.world_state.is_empty());
    }

    /// A pending-candidate fixture for the shared (`user_id = ""`) scope,
    /// which is visible to any session user.
    fn pending_candidate_row() -> ene_core::PendingCandidate {
        ene_core::PendingCandidate {
            id: 0,
            character_id: "default".into(),
            user_id: String::new(),
            kind: ene_core::MemoryKind::Preference,
            title: "cats".into(),
            content: "user dislikes cats".into(),
            confidence: 0.9,
            reason_detail: "test fixture".into(),
            existing_memory_title: None,
            existing_memory_id: None,
            source_quote: "test".into(),
            source_turn: None,
            outcome_rating: None,
            approval_parked: false,
            status: ene_core::PendingCandidateStatus::Pending,
            created_at: chrono::Utc::now() - chrono::Duration::days(5),
            resolved_at: None,
        }
    }

    fn proactive_with_pending_confirmation(mind: &mut ene_mind::MindConfig) {
        mind.proactive.enabled = true;
        mind.proactive.min_idle_seconds = 0;
        mind.proactive.cooldown_seconds = 0;
        mind.proactive.sources = ene_mind::ProactiveSourcesConfig {
            conversation: false,
            activity: false,
            screen_summary: false,
            memory: false,
            ..ene_mind::ProactiveSourcesConfig::default()
        };
        mind.proactive.pending_confirmation.enabled = true;
        mind.proactive.pending_confirmation.min_age_days = 0;
        mind.proactive.pending_confirmation.min_confidence = 0.0;
    }

    async fn actor_with_pending_store() -> (
        TurnActor,
        broadcast::Receiver<DiagnosticEvent>,
        broadcast::Receiver<LifecycleEvent>,
        Arc<ene_store::MemoryStore>,
    ) {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor_with_session_and_gate};

        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, diag_rx, lifecycle_rx, _gate, _shared) =
            build_bare_actor_with_session_and_gate(
                registry,
                &task_caps,
                ConversationSession::new(),
                EneConfig::default(),
            );
        actor.concrete_store = Some(Arc::clone(&store));
        actor.session.memory.memory_store =
            Some(Arc::clone(&store) as Arc<dyn ene_core::MemoryPort>);
        let mut mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");
        proactive_with_pending_confirmation(&mut mind);
        actor
            .config
            .set_section(&mind)
            .expect("set mind config on test actor");
        (actor, diag_rx, lifecycle_rx, store)
    }

    #[tokio::test]
    async fn pending_reply_approval_resolves_through_the_store() {
        let (mut actor, _diag_rx, mut lifecycle_rx, store) = actor_with_pending_store().await;
        let candidate_id = store
            .insert_pending_candidate(pending_candidate_row())
            .await
            .expect("insert fixture");
        let cache = actor
            .session
            .memory
            .recall_cache
            .clone()
            .expect("recall cache");
        let cached = cache
            .list_pending_candidates(store.as_ref(), "default")
            .await
            .expect("prime the cache");
        assert_eq!(cached.len(), 1);

        let turn = TurnId::new();
        actor.proactive.pending_confirmation_asked_at.insert(
            candidate_id,
            (actor.quiet_hours_clock)() - chrono::Duration::days(1),
        );
        let result = crate::proactive::PendingResolutionResult {
            session_epoch: actor.proactive.session_epoch,
            candidate_id,
            turn: turn.clone(),
            verdict: ene_mind::PendingResolutionVerdict::Approved,
            detail: String::new(),
        };
        actor.handle_proactive_resolution(result).await;

        let rows = store
            .list_pending_candidates("default", None)
            .await
            .expect("list all rows");
        let row = rows
            .iter()
            .find(|row| row.id == candidate_id)
            .expect("fixture row present");
        assert_eq!(row.status, ene_core::PendingCandidateStatus::Approved);
        assert!(
            !actor
                .proactive
                .pending_confirmation_asked_at
                .contains_key(&candidate_id),
            "a resolved candidate no longer needs re-ask backoff"
        );
        let after = cache
            .list_pending_candidates(store.as_ref(), "default")
            .await
            .expect("re-list through the cache");
        assert!(
            after.is_empty(),
            "approval must invalidate the recall cache"
        );
        assert!(
            matches!(
                lifecycle_rx.try_recv(),
                Ok(LifecycleEvent::CandidateChanged { id, status, turn: Some(emitted_turn) })
                    if id == candidate_id
                        && status == ene_store::PendingCandidateStatus::Approved
                        && emitted_turn == turn
            ),
            "approval must emit CandidateChanged for the classified turn"
        );
    }

    #[tokio::test]
    async fn pending_reply_rejection_resolves_through_the_store() {
        let (mut actor, _diag_rx, mut lifecycle_rx, store) = actor_with_pending_store().await;
        let candidate_id = store
            .insert_pending_candidate(pending_candidate_row())
            .await
            .expect("insert fixture");

        actor.proactive.pending_confirmation_asked_at.insert(
            candidate_id,
            (actor.quiet_hours_clock)() - chrono::Duration::days(1),
        );
        let result = crate::proactive::PendingResolutionResult {
            session_epoch: actor.proactive.session_epoch,
            candidate_id,
            turn: TurnId::new(),
            verdict: ene_mind::PendingResolutionVerdict::Rejected,
            detail: String::new(),
        };
        actor.handle_proactive_resolution(result).await;

        let rows = store
            .list_pending_candidates("default", None)
            .await
            .expect("list all rows");
        let row = rows
            .iter()
            .find(|row| row.id == candidate_id)
            .expect("fixture row present");
        assert_eq!(row.status, ene_core::PendingCandidateStatus::Rejected);
        assert!(
            !actor
                .proactive
                .pending_confirmation_asked_at
                .contains_key(&candidate_id),
            "a resolved candidate no longer needs re-ask backoff"
        );
        assert!(
            matches!(
                lifecycle_rx.try_recv(),
                Ok(LifecycleEvent::CandidateChanged { id, status, .. })
                    if id == candidate_id
                        && status == ene_store::PendingCandidateStatus::Rejected
            ),
            "rejection must emit CandidateChanged"
        );
    }

    #[tokio::test]
    async fn unclear_reply_leaves_the_candidate_pending() {
        let (mut actor, _diag_rx, mut lifecycle_rx, store) = actor_with_pending_store().await;
        let candidate_id = store
            .insert_pending_candidate(pending_candidate_row())
            .await
            .expect("insert fixture");

        actor.proactive.pending_confirmation_asked_at.insert(
            candidate_id,
            (actor.quiet_hours_clock)() - chrono::Duration::days(1),
        );
        let result = crate::proactive::PendingResolutionResult {
            session_epoch: actor.proactive.session_epoch,
            candidate_id,
            turn: TurnId::new(),
            verdict: ene_mind::PendingResolutionVerdict::Unclear,
            detail: "unclear".into(),
        };
        actor.handle_proactive_resolution(result).await;

        let rows = store
            .list_pending_candidates("default", None)
            .await
            .expect("list all rows");
        let row = rows
            .iter()
            .find(|row| row.id == candidate_id)
            .expect("fixture row present");
        assert_eq!(row.status, ene_core::PendingCandidateStatus::Pending);
        assert!(
            actor
                .proactive
                .pending_confirmation_asked_at
                .contains_key(&candidate_id),
            "an unclear verdict must keep the backoff armed"
        );
        assert!(
            lifecycle_rx.try_recv().is_err(),
            "an unclear verdict must not emit CandidateChanged"
        );
    }

    #[tokio::test]
    async fn resolution_spawn_without_handles_restores_the_marker() {
        let (mut actor, _diag_rx, _lifecycle_rx, _store) = actor_with_pending_store().await;
        actor.proactive.asked_pending_candidate = Some(ene_mind::PendingConfirmationPrompt {
            id: 1,
            title: "cats".into(),
            content: "user dislikes cats".into(),
            age_days: 5.0,
        });
        actor.active_turn = Some(TurnId::new());
        actor.session.add_user_message("yes, still true");

        actor.spawn_pending_resolution();

        assert!(
            actor.proactive_resolution_rx.is_none(),
            "no classification may spawn without LLM handles"
        );
        assert!(
            actor.proactive.asked_pending_candidate.is_some(),
            "the marker must survive so the next user turn retries"
        );
    }

    #[tokio::test]
    async fn quiet_hours_queue_a_due_pending_confirmation_as_an_opportunity() {
        use chrono::TimeZone;

        let (mut actor, _diag_rx, _lifecycle_rx, store) = actor_with_pending_store().await;
        store
            .insert_pending_candidate(pending_candidate_row())
            .await
            .expect("insert fixture");
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Queue,
            ene_mind::QuietHoursSuppressConfig {
                decisions: true,
                ..ene_mind::QuietHoursSuppressConfig::default()
            },
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        // The quiet-hours helper resets the mind config; re-apply the
        // pending-confirmation trigger on top of it.
        let mut mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");
        proactive_with_pending_confirmation(&mut mind);
        actor
            .config
            .set_section(&mind)
            .expect("set mind config on test actor");

        actor.maybe_spawn_proactive_decision().await;

        assert!(
            actor.proactive_decision_rx.is_none(),
            "no decision task may spawn during quiet hours"
        );
        assert_eq!(
            actor.proactive.quiet_hours_queue.len(),
            1,
            "a due pending candidate alone is a real quiet-hours opportunity"
        );
    }

    #[tokio::test]
    async fn pending_selection_skips_while_a_question_is_outstanding() {
        let (mut actor, _diag_rx, _lifecycle_rx, store) = actor_with_pending_store().await;
        let candidate_id = store
            .insert_pending_candidate(pending_candidate_row())
            .await
            .expect("insert fixture");
        let mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");

        let (_, _, _, _, _, selected) = actor.proactive_context_inputs(&mind).await;
        assert!(
            selected.is_some(),
            "a due candidate must be selected when nothing is asked"
        );

        actor.proactive.asked_pending_candidate = Some(ene_mind::PendingConfirmationPrompt {
            id: 1,
            title: "cats".into(),
            content: "user dislikes cats".into(),
            age_days: 5.0,
        });
        let (_, _, _, _, _, selected) = actor.proactive_context_inputs(&mind).await;
        assert!(
            selected.is_none(),
            "selection must skip while a confirmation question is in flight"
        );

        // The backoff survives the marker: an unclear reply consumed the
        // marker, but the delivered question still blocks re-selection.
        actor.proactive.asked_pending_candidate = None;
        actor.proactive.pending_confirmation_asked_at.insert(
            candidate_id,
            (actor.quiet_hours_clock)() - chrono::Duration::days(1),
        );
        let (_, _, _, _, _, selected) = actor.proactive_context_inputs(&mind).await;
        assert!(
            selected.is_none(),
            "a recently asked candidate must wait out the re-ask backoff"
        );

        actor.proactive.pending_confirmation_asked_at.insert(
            candidate_id,
            (actor.quiet_hours_clock)() - chrono::Duration::days(8),
        );
        let (_, _, _, _, _, selected) = actor.proactive_context_inputs(&mind).await;
        assert!(
            selected.is_some(),
            "after the backoff window the candidate is due again"
        );
    }

    #[tokio::test]
    async fn quiet_hours_suppress_notifications_skips_running_and_idle() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Discard,
            ene_mind::QuietHoursSuppressConfig {
                notifications: true,
                decisions: false,
                tts: false,
            },
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        let mut lifecycle_rx = actor.lifecycle_tx.subscribe();
        let result = actor.synthetic_catch_up_result("test");
        let started = actor
            .begin_proactive_generation(&result, "test hint".to_string())
            .await;

        assert!(
            !started,
            "the bare actor cannot open a provider, so the stream must not start"
        );
        assert!(
            lifecycle_rx.try_recv().is_err(),
            "no Running or Idle announcement may be emitted while notifications are suppressed"
        );
        assert!(
            !actor.quiet_hours_notifications_suppressed,
            "the suppression flag must reset when no stream runs"
        );
    }

    #[tokio::test]
    async fn proactive_generation_announces_running_and_idle_when_not_suppressed() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Discard,
            ene_mind::QuietHoursSuppressConfig::default(),
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 23, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        let mut lifecycle_rx = actor.lifecycle_tx.subscribe();
        let result = actor.synthetic_catch_up_result("test");
        let started = actor
            .begin_proactive_generation(&result, "test hint".to_string())
            .await;

        assert!(!started);
        assert!(matches!(
            lifecycle_rx.try_recv(),
            Ok(LifecycleEvent::StatusChanged {
                status: EneStatus::Running
            })
        ));
        assert!(matches!(
            lifecycle_rx.try_recv(),
            Ok(LifecycleEvent::StatusChanged {
                status: EneStatus::Idle
            })
        ));
        assert!(lifecycle_rx.try_recv().is_err());
    }

    struct StubTtsProvider;

    #[async_trait::async_trait]
    impl ene_ai::TtsProvider for StubTtsProvider {
        fn name(&self) -> &'static str {
            "stub-tts"
        }

        async fn synthesize_stream(
            &self,
            _text: &str,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<ene_ai::TtsChunk, ene_ai::AudioProviderError>,
                        > + Send,
                >,
            >,
            ene_ai::AudioProviderError,
        > {
            Ok(Box::pin(tokio_stream::empty::<
                Result<ene_ai::TtsChunk, ene_ai::AudioProviderError>,
            >()))
        }
    }

    #[tokio::test]
    async fn quiet_hours_suppress_tts_drops_the_provider_for_proactive_turns() {
        use crate::handle::tests::{EmptyRegistry, build_bare_actor};
        use chrono::TimeZone;

        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);
        quiet_hours_actor(
            &mut actor,
            ene_mind::QuietHoursPolicy::Discard,
            ene_mind::QuietHoursSuppressConfig {
                notifications: false,
                decisions: false,
                tts: true,
            },
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 22, 30, 0)
                .single()
                .expect("valid utc instant"),
        );
        actor.tts_provider = Some(Arc::new(StubTtsProvider));

        let mind = actor
            .config
            .get_section::<ene_mind::MindConfig>()
            .expect("mind config");
        let quiet = evaluate_quiet_hours(&mind.proactive.quiet_hours, (actor.quiet_hours_clock)());
        assert!(
            actor
                .proactive_tts_provider(&quiet, mind.proactive.quiet_hours.suppress)
                .is_none(),
            "TTS must be dropped while quiet hours suppress speech audio"
        );

        actor.quiet_hours_clock = Arc::new(|| {
            chrono::Utc
                .with_ymd_and_hms(2026, 8, 3, 23, 30, 0)
                .single()
                .expect("valid utc instant")
        });
        let quiet = evaluate_quiet_hours(&mind.proactive.quiet_hours, (actor.quiet_hours_clock)());
        assert!(
            actor
                .proactive_tts_provider(&quiet, mind.proactive.quiet_hours.suppress)
                .is_some(),
            "TTS must stay available outside the quiet-hours window"
        );
    }
}
