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
//! Config/control operations (`SetCharacter`, `UpdateFeatureSettings`,
//! plugin host restart) stay on this actor rather than a separate control
//! plane: they are infrequent, low-latency, and never block behind a `Run`
//! turn any worse than `Run` itself already does.

use super::SharedActorState;
use super::TurnGate;
use super::command::{DeferredToolTask, EneCommand, FeatureSettingsUpdate};
use super::event::{
    AudioChunk, EneEvent, EneStateSnapshot, EneStatus, LifecycleEvent, TerminalReason,
};
use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::error::EneRuntimeError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::{RequestId, TurnId};
use crate::vision::VisionPrepared;
use ene_ai::{AiTaskKind, LlmProviderRegistry, create_task_chat_provider};
use ene_config::EneConfig;
#[cfg(any(unix, windows))]
use ene_connector::CredentialDeclaration;
#[cfg(any(unix, windows))]
use ene_connector::CredentialStore;
#[cfg(any(unix, windows))]
use ene_connector::declaration::{CredentialKind, ScopeDecision, resolve_scope};
#[cfg(any(unix, windows))]
use ene_connector::vault::{CredentialVault, VaultEntry};
use ene_mind::commitments::CommitmentLedger;
use ene_mind::{CardName, SessionId};
use ene_mind::{
    CompressionLevel, CompressionResult, CompressionTaskInput, HistoryEntry as MindHistoryEntry,
    compression_has_usable_summary,
};
use ene_mind::{ConversationSession, EneSessionError};
use ene_plugin_host::CredentialRegistry;
use ene_plugin_host::credential_service::{CredentialPassenger, CredentialPluginRegistration};
#[cfg(any(unix, windows))]
use ene_plugin_host::oauth::{
    CredentialPersister, FileCredentialPersister, OAuthFlowManager, OAuthRefresher,
};
use ene_plugin_host::{
    CompositeToolRegistry, DisabledReason, PluginHealthEvent, PluginHostError, ToolRegistry,
};
#[cfg(any(unix, windows))]
use ene_plugin_proto::{HostServiceId, HostServicePassenger};
use ene_rag::{ToolRag, ToolRagConfig, ToolRagOptions};
#[cfg(any(unix, windows))]
use ene_store::host_service::{DbPluginRegistration, HostServiceServer, host_service_socket_path};
use once_cell::sync::OnceCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

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
    session: ConversationSession,
    /// Concrete store for IPC server (`connection()`) and `ToolRag`.
    ///
    /// `session.memory.memory_store` holds the same store as `Arc<dyn MemoryPort>`
    /// for `ene-mind`; this field keeps the concrete type for callers that need
    /// `MemoryStore`-specific methods not on the trait.
    concrete_store: Option<Arc<ene_store::MemoryStore>>,
    registry: Arc<dyn ToolRegistry>,
    tool_rag: Option<Arc<ToolRag>>,
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
    permission_scopes: Arc<Mutex<Vec<crate::streaming::PermissionScope>>>,
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
    /// Local / cloud decision provider handles (lazy).
    ///
    /// Shared via `Arc` so background init tasks can populate it without
    /// blocking the actor loop. The `OnceCell` guarantees at-most-once
    /// initialization; `proactive_llm_init_spawned` prevents duplicate
    /// background spawns.
    proactive_llm: Arc<OnceCell<ene_ai_local::ProactiveLlmHandles>>,
    /// Guards against spawning multiple background init tasks for
    /// [`Self::proactive_llm`]. Set to `true` when a background load
    /// is spawned; reset to `false` if that load *fails* so a later call can
    /// retry — a single transient failure must not permanently disable
    /// proactive features for the process lifetime. On success the `OnceCell`
    /// is populated, so the `get().is_some()` fast-path short-circuits before
    /// this flag is consulted again. Shared via `Arc` so the background init
    /// task can reset it on failure.
    proactive_llm_init_spawned: Arc<AtomicBool>,
    /// Guards against invoking [`ene_ai_local::ProactiveLlmHandles::shutdown`]
    /// more than once. Both the `EneCommand::Shutdown` arm and the
    /// post-loop cleanup in [`Self::run`] run on every normal teardown from
    /// unsynchronized call sites, and `Arc<OnceCell>` cannot be consumed
    /// through a shared reference to deduplicate them, so this flag provides
    /// the at-most-once guarantee: a `shutdown()` with real side effects
    /// (unloading GGUF weights, releasing GPU memory) must never be
    /// double-invoked.
    proactive_llm_shutdown: AtomicBool,
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
    /// Declaration registry shared with the plugin host manager; retained so
    /// a config change can rebuild the credential vault from the populated
    /// declarations. `None` only before the first reconfiguration settles.
    credential_registry: Option<Arc<CredentialRegistry>>,
    /// The host-service `credential` passenger; `None` when no host-service
    /// endpoint was bound. Retained so the runtime can swap a rebuilt vault
    /// in without restarting the host.
    credential_passenger: Option<Arc<CredentialPassenger>>,
    /// Mailbox-free shared actor state: card name (shared with
    /// [`crate::query::candidates::MemoryCandidateHandle`]),
    /// session id / started-at / turn count, config, and the loaded card.
    /// [`crate::EneHandle`] reads these slots synchronously; this actor keeps
    /// them in sync at the mutation points (session split, `SetCharacter`,
    /// per-turn bookkeeping, feature-settings updates).
    shared: Arc<SharedActorState>,
}

impl TurnActor {
    /// Constructs a ready `TurnActor`. Called once from [`crate::EneHandle::open`].
    #[expect(
        clippy::too_many_arguments,
        reason = "single internal constructor call site (EneHandle::open); the #272 \
                  event-bus split added two channel senders (lifecycle_tx, audio_tx) \
                  and #397 added cmd_tx for internal command feedback, pushing this \
                  from 17 to 18 — grouping into a config struct is a larger refactor \
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
        health_monitor: ene_ai::ProviderHealthMonitor,
        tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
        plugin_tool_registries: Vec<Arc<dyn ToolRegistry>>,
        plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
        health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        credential_registry: Option<Arc<CredentialRegistry>>,
        credential_passenger: Option<Arc<CredentialPassenger>>,
        shared_state: Arc<SharedActorState>,
    ) -> Self {
        let (classifier_tx, classifier_rx) = mpsc::unbounded_channel();
        let (memory_writer_tx, memory_writer_rx) = mpsc::unbounded_channel();
        let (deferred_tool_tx, deferred_tool_rx) = mpsc::unbounded_channel();
        let (aux_task_tx, aux_task_rx) = mpsc::unbounded_channel();
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
            session,
            concrete_store,
            registry,
            tool_rag,
            cancel_token: CancellationToken::new(),
            stream_handle: None,
            stream_session_rx: None,
            stream_partial_text: Arc::new(parking_lot::Mutex::new(String::new())),
            active_turn: None,
            vision_cancel: CancellationToken::new(),
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            permission_scopes: Arc::new(Mutex::new(Vec::new())),
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
            proactive_llm: Arc::new(OnceCell::new()),
            proactive_llm_init_spawned: Arc::new(AtomicBool::new(false)),
            proactive_llm_shutdown: AtomicBool::new(false),
            vision_slow_path_active: Arc::new(AtomicBool::new(false)),
            active_origin: crate::types::TurnOrigin::User,
            health_monitor,
            task_caps,
            tts_provider,
            plugin_tool_registries,
            plugin_host,
            health_bridge_handle,
            host_service_handle,
            credential_registry,
            credential_passenger,
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

    pub(super) async fn run(mut self) {
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
                                && outcome.terminal == TerminalReason::Done
                            {
                                self.proactive.on_proactive_completed();
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
                        drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                            status: EneStatus::Idle,
                        }));
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
        self.shutdown_proactive_llm_once().await;
        // Cooperative cancel first: aux workers watching the turn's token
        // (the TTS pipeline) exit gracefully instead of being force-aborted.
        self.cancel_token.cancel();
        // Admit aux handles that arrived after the run loop's last drain so
        // teardown aborts them rather than dropping them (which would only
        // detach, not cancel, the worker).
        self.admit_aux_handles();
        self.abort_all_join_sets();
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

    /// Lists every stored credential plus every currently declared `OAuth2`
    /// credential, as non-secret summaries.
    #[cfg(any(unix, windows))]
    fn list_credentials(&self) -> Vec<ene_plugin_host::oauth::CredentialInfo> {
        let Some(passenger) = &self.credential_passenger else {
            return Vec::new();
        };
        match passenger.oauth_flow() {
            Some(flow) => flow.list_credentials(),
            None => Vec::new(),
        }
    }

    /// Revokes credentials by storage key: vault entries are dropped, the
    /// persistence file entry is removed, and clients are invalidated.
    #[cfg(any(unix, windows))]
    fn revoke_credential(&self, ids: &[String]) -> usize {
        let Some(passenger) = &self.credential_passenger else {
            return 0;
        };
        let Some(flow) = passenger.oauth_flow() else {
            return 0;
        };
        flow.revoke(ids).unwrap_or_else(|e| {
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Credential revocation failed"
            );
            0
        })
    }

    /// Starts the OAuth authorization flow for `id` (desktop settings page);
    /// the flow completes out-of-band.
    #[cfg(any(unix, windows))]
    fn authorize_credential(&self, id: &str) -> Result<(), String> {
        let Some(passenger) = &self.credential_passenger else {
            return Err(
                "no plugin host is running; cannot start an authorization flow".to_string(),
            );
        };
        match passenger.oauth_flow() {
            Some(flow) => flow
                .start_authorization_by_id(id)
                .map_err(|e| e.to_string()),
            None => {
                Err("no plugin host is running; cannot start an authorization flow".to_string())
            }
        }
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

        let ai_cfg = match self.config.get_section::<ene_ai::AiConfig>() {
            Ok(cfg) => cfg,
            Err(e) => {
                // Reset so a later call (e.g. after a config hot-reload fixes
                // the section) can retry rather than being permanently stuck.
                self.proactive_llm_init_spawned
                    .store(false, Ordering::Release);
                tracing::warn!(
                    component = "Proactive",
                    error = %e,
                    "Cannot init proactive LLM: config error"
                );
                return;
            }
        };
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
        self.bg_command_tasks.spawn(async move {
            match ene_ai_local::build_proactive_llm_handles(&ai_cfg).await {
                Ok(handles) => {
                    tracing::info!(
                        component = "Proactive",
                        decision_backend = ?handles.decision_kind,
                        vision = handles
                            .local()
                            .is_some_and(|l| l.capabilities().contains(ene_ai::Capability::Vision)),
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

    /// Shuts down the proactive LLM handles at most once.
    ///
    /// Both the `EneCommand::Shutdown` arm and the post-loop cleanup in
    /// [`Self::run`] call this on every normal teardown; the
    /// `proactive_llm_shutdown` flag guarantees the underlying
    /// [`ene_ai_local::ProactiveLlmHandles::shutdown`] runs only on the first
    /// of the two, since `Arc<OnceCell>` cannot be consumed through a shared
    /// reference.
    async fn shutdown_proactive_llm_once(&self) {
        if let Some(handles) = self.proactive_llm.get()
            && !self.proactive_llm_shutdown.swap(true, Ordering::AcqRel)
        {
            handles.shutdown().await;
        }
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
            let result = finish_vision_prep(handles, &prompt_language, &app_label);
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
                    let result =
                        finish_vision_prep(handles, &prompt_language, &app_label).map(|mut vp| {
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
        let observation = self.proactive.observation.clone();
        let history = self.session.history().to_vec();
        let card_name = self.session.card_name().to_string();
        let user_name = self.config.user_name.clone();
        let mem_store = self.session.memory.memory_store.clone();
        let (affect, commitments) = if let Some(store) = mem_store.as_ref() {
            let affect = store.get_affect_state(&card_name).await.ok();
            let raw = store
                .list_active_commitments(&card_name, Some(user_name.as_str()), 10)
                .await
                .unwrap_or_default();
            let commitments = CommitmentLedger::active_prompt_candidates(&raw);
            (affect, commitments)
        } else {
            (None, Vec::new())
        };
        let (tx, rx) = oneshot::channel();
        self.proactive_decision_rx = Some(rx);
        let config = mind.proactive.clone();
        let prompt_language = mind.resolved_classifier_language().to_owned();
        let handle = tokio::spawn(async move {
            let result = crate::proactive::run_decision_task(
                config,
                history,
                observation,
                suppression,
                decision_provider,
                epoch,
                affect,
                commitments,
                prompt_language,
            )
            .await;
            drop(tx.send(result));
        });
        self.proactive_decision_handle = Some(handle);
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

        let turn = TurnId::new();
        if !self.turn_gate.try_begin(&turn) {
            tracing::info!(
                component = "Proactive",
                speak = false,
                detail = "turn gate busy",
                "Proactive will not speak"
            );
            return;
        }
        tracing::info!(
            component = "Proactive",
            speak = true,
            confidence = result.confidence,
            topic_hint = %result.topic_hint,
            detail = %result.detail,
            "Proactive will speak"
        );
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        let hint = crate::proactive::proactive_generation_hint(
            &result.topic_hint,
            mind.resolved_classifier_language(),
        );
        let screen_image = self
            .config
            .get_section::<ene_ai::AiConfig>()
            .ok()
            .filter(ene_ai::AiConfig::proactive_generation_supports_vision)
            .and_then(|_| self.proactive.take_screen_image());
        if screen_image.is_none() {
            // Drop any stashed frame when the generation model cannot use it.
            let _ = self.proactive.take_screen_image();
        }
        self.drain_pending().await;
        self.cancel_token = CancellationToken::new();
        self.terminal_emitted = Arc::new(AtomicBool::new(false));
        self.active_turn = Some(turn.clone());
        self.active_origin = crate::types::TurnOrigin::Proactive;
        drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
            status: EneStatus::Running,
        }));
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
            Some(generation_timeout),
        )
        .await;
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
                drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                    status: EneStatus::Idle,
                }));
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
                self.shutdown_proactive_llm_once().await;
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
                drop(reply.send(Ok(())));
                true
            }
            EneCommand::UpdateProactiveObservation { observation } => {
                self.proactive.observation = observation;
                // When screen_summary is enabled, each observe cycle (fresh
                // capture → vision) drives the decision LLM immediately.
                let screen_driven = self
                    .config
                    .get_section::<ene_mind::MindConfig>()
                    .is_ok_and(|m| m.proactive.enabled && m.proactive.sources.screen_summary);
                if screen_driven {
                    self.maybe_spawn_proactive_decision().await;
                }
                true
            }
            EneCommand::UpdateProactiveSettings { mind } => {
                if let Ok(mut mind_cfg) = self.config.get_section::<ene_mind::MindConfig>() {
                    mind_cfg.proactive = mind;
                    drop(self.config.set_section(&mind_cfg));
                    self.sync_shared_config();
                }
                self.abort_proactive_decision();
                true
            }
            EneCommand::UpdateFeatureSettings { settings } => {
                let FeatureSettingsUpdate {
                    mind,
                    store,
                    plugins,
                    rag,
                } = *settings;
                let prev_plugins = self
                    .config
                    .get_section::<ene_plugin_host::PluginConfig>()
                    .unwrap_or_default();
                let plugins_changed = plugin_enable_set_changed(&prev_plugins, &plugins);
                let config_updates = if plugins_changed {
                    HashMap::new()
                } else {
                    plugin_config_blob_updates(&prev_plugins, &plugins)
                };

                drop(self.config.set_section(&mind));
                drop(self.config.set_section(&store));
                drop(self.config.set_section(&plugins));
                drop(self.config.set_section(&rag));
                self.sync_shared_config();
                self.abort_proactive_decision();

                if plugins_changed {
                    // The enabled plugin set changed: restart the plugin host
                    // so newly-enabled plugins are spawned and disabled ones
                    // are stopped, then rebuild the tool registry from the new
                    // host. Runs in bg_command_tasks so the actor loop is
                    // never blocked on process I/O.
                    self.spawn_reconfigure_plugin_host();
                } else if !config_updates.is_empty() {
                    // Enable-set unchanged but config/profiles blobs changed:
                    // push SetConfig to live connections (and refresh their
                    // reconnect cache) without a full host restart.
                    self.spawn_push_plugin_configs(config_updates);
                    // The credential vault is a config snapshot too: a rotated
                    // `api_key` or new provider must reach the credential
                    // service even though the plugin enable-set did not move.
                    if let Some(passenger) = self.credential_passenger.as_ref()
                        && let Some(registry) = self.credential_registry.as_ref()
                    {
                        refresh_credential_vault(&self.config, registry, passenger);
                    }
                }
                true
            }
            EneCommand::PrepareVisionSummary { app_label, reply } => {
                self.prepare_vision_summary(app_label, reply);
                true
            }
            EneCommand::StashProactiveScreenImage { data_uri } => {
                self.proactive.last_screen_image_data_uri = data_uri;
                true
            }
            EneCommand::PluginHostReconfigured {
                registry,
                plugin_tool_registries,
                credential_registry,
                credential_passenger,
            } => {
                self.registry = registry;
                self.plugin_tool_registries = plugin_tool_registries;
                self.credential_registry = Some(credential_registry);
                self.credential_passenger = credential_passenger;
                true
            }
            EneCommand::PermissionDecision {
                request_id,
                decision,
            } => {
                let mut guard = self.pending_permissions.lock().await;
                if let Some(tx) = guard.remove(&request_id) {
                    // A oneshot `Sender<PermissionDecision>::send` error is
                    // `Copy` (it's just the unsent value), so `drop()` would
                    // itself trip `clippy::dropping_copy_types`; a dropped
                    // receiver just means the caller stopped waiting.
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                    )]
                    let _ = tx.send(decision);
                }
                drop(guard);
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
            #[cfg(any(unix, windows))]
            EneCommand::ListCredentials { reply } => {
                let infos = self.list_credentials();
                drop(reply.send(infos));
                true
            }
            #[cfg(any(unix, windows))]
            EneCommand::RevokeCredential { ids, reply } => {
                let removed = self.revoke_credential(&ids);
                // A oneshot send error is `Copy` here (it's just the unsent
                // `usize`), so `drop()` would itself trip
                // `clippy::dropping_copy_types`; a dropped receiver just
                // means the caller stopped waiting.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "oneshot send error is Copy; drop() would trip dropping_copy_types"
                )]
                let _ = reply.send(removed);
                true
            }
            #[cfg(any(unix, windows))]
            EneCommand::AuthorizeCredential { id, reply } => {
                let result = self.authorize_credential(&id);
                drop(reply.send(result));
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
                    let context = turn.as_ref().map(|turn| ene_plugin_proto::CallContext {
                        conversation_id: session_id,
                        turn_id: turn.to_string(),
                    });
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
                            .call_tool(&name, &arguments, context.as_ref())
                            .await
                            .map(|r| r.text_for_llm())
                            .map_err(EneRuntimeError::from)
                    };
                    drop(reply.send(result));
                });
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
        generation_timeout: Option<std::time::Duration>,
    ) {
        // Create the provider before mutating history so a failed open leaves
        // the session unchanged.
        let provider = match if origin == crate::types::TurnOrigin::Proactive {
            self.create_proactive_provider()
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
                drop(self.lifecycle_tx.send(LifecycleEvent::StatusChanged {
                    status: EneStatus::Idle,
                }));
                return;
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
        let tts_provider = self.tts_provider.clone();
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
    }

    async fn create_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        let ai_config = self
            .config
            .get_section::<ene_ai::AiConfig>()
            .unwrap_or_default();

        // Fast path: failover disabled → use the configured chat task directly.
        if !ai_config.fallback.enabled {
            return create_task_chat_provider(&self.config, AiTaskKind::Chat)
                .map(Arc::from)
                .map_err(EneRuntimeError::from);
        }

        // Failover path: probe candidates in priority order and pick the first
        // healthy one. Probes send no user data.
        let candidates = ai_config.resolve_chat_candidates();
        if candidates.is_empty() {
            return create_task_chat_provider(&self.config, AiTaskKind::Chat)
                .map(Arc::from)
                .map_err(EneRuntimeError::from);
        }

        let timeout = std::time::Duration::from_millis(ai_config.fallback.health_check_timeout_ms);
        let selection = ene_ai::select_healthy_chat(&candidates, &self.health_monitor, timeout)
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

        let resolved = selection.candidate.to_resolved();
        let provider = ene_ai::create_chat_provider_from_resolved(&resolved)
            .with_retry_policy(ai_config.retry.to_policy());
        Ok(Arc::new(provider))
    }

    fn create_proactive_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        create_task_chat_provider(&self.config, AiTaskKind::Proactive)
            .map(Arc::from)
            .map_err(EneRuntimeError::from)
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

    // ── Split management ──

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
    /// Called after every in-place config mutation (`UpdateFeatureSettings`,
    /// `UpdateProactiveSettings`) so [`crate::EneHandle::config`] stays
    /// consistent with the actor's authoritative copy. Read-only sharing:
    /// only the actor writes, and only under a write lock.
    fn sync_shared_config(&self) {
        let cfg = Arc::new(self.config.clone());
        *self.shared.config.write() = cfg;
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
    handles: &ene_ai_local::ProactiveLlmHandles,
    prompt_language: &str,
    app_label: &str,
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
    if !local.capabilities().contains(ene_ai::Capability::Vision) {
        return Err(PublicApiError::Internal {
            message: "local model has no vision mmproj loaded".to_string(),
        });
    }

    let prompts = ene_config::PromptLibrary::load(prompt_language);
    let system = prompts.proactive().screen_summary_system.trim().to_string();
    let user = prompts.proactive().render_screen_summary_user(app_label);
    Ok(VisionPrepared {
        local,
        system,
        user,
        cancel: CancellationToken::new(), // Replaced by caller.
    })
}

fn stale_llm_factory_names(
    old_kinds: &[String],
    new_factories: Option<&HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>>>,
) -> Vec<String> {
    old_kinds
        .iter()
        .filter(|kind| new_factories.is_none_or(|factories| !factories.contains_key(*kind)))
        .cloned()
        .collect()
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
    // Stop the previous host (and its health bridge) first. Keep the factory
    // handles so stale removal below cannot evict a replacement installed by
    // another runtime handle.
    let old_llm_factories: HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>> = {
        let mut guard = plugin_host.lock().await;
        let factories = guard.as_ref().map_or_else(HashMap::new, |host| {
            host.llm_factories()
                .iter()
                .map(|(kind, factory)| (kind.clone(), Arc::clone(factory)))
                .collect()
        });
        if let Some(mut host) = guard.take() {
            host.shutdown().await;
        }
        drop(guard);

        let mut bridge = health_bridge_handle.lock().await;
        if let Some(handle) = bridge.take() {
            handle.abort();
        }
        factories
    };

    // Abort the previous host-service accept loop before rebinding the shared
    // socket path, or the stale listener keeps serving an unlinked socket
    // (unix) and the Windows rebind fails while the old pipe instance lives.
    if let Some(handle) = host_service_handle.lock().await.take() {
        handle.abort();
    }

    // The credential registry is shared between the host-service passenger
    // and the new host: the passenger is built by spawn_host_services below,
    // and the new host populates the registry from plugin schemas during
    // start (before any plugin can open a credential session).
    let credential_registry = Arc::new(CredentialRegistry::new());
    let (db_tokens, credential_tokens, credential_passenger) = match spawn_host_services(
        &config,
        memory_store.as_ref(),
        Arc::clone(&credential_registry),
    ) {
        Ok(services) => {
            *host_service_handle.lock().await = services.handle;
            (
                services.db_tokens,
                services.credential_tokens,
                services.credential_passenger,
            )
        }
        Err(e) => {
            *host_service_handle.lock().await = None;
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Failed to spawn host service during plugin reconfiguration; \
                 continuing without plugin DB/credential access"
            );
            (HashMap::new(), HashMap::new(), None)
        }
    };

    // Start the new host before removing old entries. A surviving provider
    // kind remains available until its replacement is registered, and stale
    // entries are removed only when the new host does not serve that kind.
    let mut new_host = match ene_plugin_host::PluginHostManager::start(
        &config,
        db_tokens,
        credential_tokens,
        Arc::clone(&credential_registry),
    )
    .await
    {
        Ok(host) => {
            // `start` populated the registry from the started plugins'
            // schemas; the interim vault built in spawn_host_services could
            // only see an empty registry, so rebuild it now (this is also
            // what surfaces private declarations) and swap it in. Plugins
            // cannot have connected yet — they only spawn from within start.
            if let Some(passenger) = credential_passenger.as_ref() {
                refresh_credential_vault(&config, &credential_registry, passenger);
            }
            Some(host)
        }
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

    let old_llm_factory_names: Vec<String> = old_llm_factories.keys().cloned().collect();
    let stale_llm_factory_names = stale_llm_factory_names(
        &old_llm_factory_names,
        new_host
            .as_ref()
            .map(ene_plugin_host::PluginHostManager::llm_factories),
    );
    let mut restore_builtin_openai = false;
    for kind in stale_llm_factory_names {
        if let Some(factory) = old_llm_factories.get(&kind)
            && LlmProviderRegistry::deregister_if_matches(&kind, factory)
        {
            tracing::info!(
                component = "TurnActor",
                kind = %kind,
                "Deregistered stale plugin-provided LLM provider factory during reconfiguration"
            );
            restore_builtin_openai |= kind == "openai-compatible";
        }
    }

    // Plugin kinds are allowed to shadow the built-in factory at registration
    // time. If that plugin disappears, restore the built-in before registering
    // any replacement from the new host.
    if restore_builtin_openai {
        LlmProviderRegistry::register(Arc::new(ene_ai::OpenAiProviderFactory));
    }

    if let Some(host) = new_host.as_ref() {
        for (kind, factory) in host.llm_factories() {
            tracing::info!(
                component = "TurnActor",
                kind = %kind,
                "Re-registered plugin-provided LLM provider factory after Features update"
            );
            LlmProviderRegistry::register(Arc::clone(factory));
        }
    }

    let mut bridge_handle: Option<tokio::task::JoinHandle<()>> = None;
    if let Some(host) = new_host.as_mut()
        && let Some(mut health_rx) = host.take_health_receiver()
    {
        let diag_tx = diag_tx.clone();
        let llm_factories_by_plugin = host.llm_factories_by_plugin();
        bridge_handle = Some(tokio::spawn(async move {
            while let Some(event) = health_rx.recv().await {
                if let PluginHealthEvent::Disabled { plugin, .. } = &event {
                    super::deregister_disabled_plugin_factories(plugin, &llm_factories_by_plugin);
                }
                emit_diag(&diag_tx, plugin_health_event_to_diag(event));
            }
        }));
    }

    let registries = new_host
        .as_ref()
        .map_or_else(Vec::new, |h| h.tool_registries().to_vec());
    let registry_count = registries.len();
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
                credential_registry,
                credential_passenger,
            }));
        }
        Err(e) => {
            tracing::warn!(
                component = "TurnActor",
                error = %e,
                "Failed to rebuild tool registry after plugin reconfiguration"
            );
        }
    }

    // Publish the new host and bridge handle to the shared slots so the
    // handle's shutdown path tears down the live instances.
    *plugin_host.lock().await = new_host;
    *health_bridge_handle.lock().await = bridge_handle;
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
            updates.insert(key.clone(), (None, None));
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

/// Plugin auth tokens plus the host-service accept-loop handle.
///
/// The handle is `None` when no plugin exists (no endpoint bound) and must be
/// aborted before re-binding on reconfiguration or shutdown.
pub(super) struct HostServices {
    /// Plugin name → `db` auth token.
    pub db_tokens: HashMap<String, String>,
    /// Plugin name → `credential` auth token.
    pub credential_tokens: HashMap<String, String>,
    /// Accept-loop handle (`None` when no plugin to serve).
    pub handle: Option<tokio::task::JoinHandle<()>>,
    /// The `credential` passenger handed to the host-service server, retained
    /// so the runtime can swap a rebuilt vault in on config change. `None`
    /// when no host-service endpoint was bound.
    pub credential_passenger: Option<Arc<CredentialPassenger>>,
}

/// Spawns the shared host-service acceptor with `db` and `credential`
/// passengers for each discovered tool plugin.
///
/// Returns per-plugin auth tokens for both services. The tokens are passed to
/// [`ene_plugin_host::PluginHostManager::start`] which hands them to the tool
/// binaries via `SandboxConfigData::db_auth_token` /
/// `credential_auth_token`.
///
/// `credential_registry` is shared with the host manager: the passenger
/// built here resolves request-time scope through it, and the manager
/// populates it from plugin schemas during `start`.
///
/// Token generation is driven by the **detected plugin set** (the binaries
/// actually discovered on disk) rather than by `plugins.list` config keys.
/// The host manager discovers plugins by scanning for `ene-plugin-{name}`
/// binaries and only consults config to skip explicitly-disabled names, so
/// keying tokens off config would orphan registrations (config key with no
/// matching binary) or starve plugins of tokens (binary with no config
/// entry). Mirroring the manager's discovery here keeps the two sets aligned.
///
/// Only the `db` passenger depends on the memory store: with storage
/// disabled no db tokens are issued (so the db service is unreachable), but
/// plugin discovery, credential tokens, the vault, and the credential
/// passenger all still work.
///
/// The returned `JoinHandle` owns the accept loop; the caller must abort it
/// before re-spawning (reconfiguration) or on shutdown so the old listener
/// does not keep serving an unlinked socket (unix) or block the pipe rebind
/// (Windows).
pub(super) fn spawn_host_services(
    config: &EneConfig,
    memory_store: Option<&Arc<ene_store::MemoryStore>>,
    credential_registry: Arc<CredentialRegistry>,
) -> Result<HostServices, EneRuntimeError> {
    let mut db_tokens = HashMap::new();
    let mut credential_tokens = HashMap::new();

    #[cfg(any(unix, windows))]
    {
        let plugin_config = config
            .get_section::<ene_plugin_host::PluginConfig>()
            .unwrap_or_default();

        // When the plugin system is disabled the host manager spawns nothing,
        // so no host-service acceptor is needed.
        if !plugin_config.enabled {
            return Ok(HostServices {
                db_tokens,
                credential_tokens,
                handle: None,
                credential_passenger: None,
            });
        }

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
        let mut credential_registrations = HashMap::new();

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

            // The db service needs a store-backed connection; without one the
            // plugin gets no db token and the db service is unreachable.
            if memory_store.is_some() {
                // Per-plugin DB storage quota. A discovered binary with no
                // config entry falls back to `PluginEntry::default()`'s quota so
                // the enforcement default applies uniformly; an explicit `null` in
                // config (`None`) disables the cap for that plugin.
                let quota_mb = match entry {
                    Some(entry) => entry.db_quota_mb,
                    None => ene_plugin_host::PluginEntry::default().db_quota_mb,
                };
                let quota_bytes = quota_mb.map(|mb| mb.saturating_mul(1024 * 1024));

                let db_auth_token = generate_host_service_token("ene-db-");
                db_tokens.insert(name.clone(), db_auth_token.clone());
                db_plugins.insert(
                    db_auth_token,
                    DbPluginRegistration {
                        tool_name: name.clone(),
                        prefix: format!("{name}_"),
                        quota_bytes,
                    },
                );
            }

            // A separate credential token keeps the two capabilities
            // independent: DB access never implies credential access. Issued
            // to every discovered plugin (like the db token) so undeclared
            // plugins can still open the service and receive `ScopeDenied`.
            let cred_auth_token = generate_host_service_token("ene-cred-");
            credential_tokens.insert(name.clone(), cred_auth_token.clone());
            credential_registrations.insert(
                cred_auth_token,
                CredentialPluginRegistration {
                    plugin: name.clone(),
                },
            );
        }

        // No plugins → no need to bind the shared endpoint.
        if db_plugins.is_empty() && credential_registrations.is_empty() {
            return Ok(HostServices {
                db_tokens,
                credential_tokens,
                handle: None,
                credential_passenger: None,
            });
        }

        // The persister is the durable home for OAuth tokens (settings.json
        // never stores them); the refresher and the flow manager both write
        // through it. All three share the same registry/declarations.
        let persister: Arc<dyn CredentialPersister> = Arc::new(FileCredentialPersister::new(
            ene_config::paths::credential_file_path(),
        ));
        let refresher: Arc<dyn ene_connector::vault::TokenRefresher> = Arc::new(
            OAuthRefresher::new(Arc::clone(&credential_registry), Arc::clone(&persister)),
        );
        let vault = Arc::new(build_credential_vault(
            config,
            &credential_registry,
            &*persister,
            Some(refresher),
        ));
        let passenger = Arc::new(CredentialPassenger::new(
            Arc::clone(&vault),
            Arc::clone(&credential_registry),
            credential_registrations,
        ));
        let oauth_flow = Arc::new(OAuthFlowManager::new(
            // The registry is moved here (its last use): refresher and
            // passenger above only borrowed it.
            credential_registry,
            Arc::clone(&vault),
            persister,
            passenger.invalidated_tx(),
        ));
        // Attach the flow to the already-shared passenger so flow completion
        // and manual revocations push through the same invalidation channel.
        passenger.set_oauth_flow(Arc::clone(&oauth_flow));
        let mut passengers: HashMap<HostServiceId, Arc<dyn HostServicePassenger>> = HashMap::new();
        passengers.insert(
            HostServiceId::Credential,
            Arc::clone(&passenger) as Arc<dyn HostServicePassenger>,
        );

        let socket_path = host_service_socket_path();
        let db = memory_store.map(|store| store.connection().clone());
        let server = HostServiceServer::new(db, socket_path, db_plugins, passengers);

        let handle = tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::error!(error = %e, "Host service server error");
            }
        });

        Ok(HostServices {
            db_tokens,
            credential_tokens,
            handle: Some(handle),
            credential_passenger: Some(passenger),
        })
    }

    #[cfg(not(any(unix, windows)))]
    {
        Ok(HostServices {
            db_tokens,
            credential_tokens,
            handle: None,
            credential_passenger: None,
        })
    }
}

/// Generates a fresh 128-bit pre-shared host-service token from the OS
/// CSPRNG (`rand::rng()`, ChaCha12-backed thread RNG seeded by the OS).
/// A 128-bit random value is collision-safe for the handful of plugins a
/// host serves; the `db` and `credential` services both draw from here so
/// there is a single generation point.
#[cfg(any(unix, windows))]
fn generate_host_service_token(prefix: &str) -> String {
    let token: u128 = rand::random();
    format!("{prefix}{token:x}")
}

/// Builds the credential vault from configuration, the declaration
/// registry, and the persistence file.
///
/// Entry keys are the storage keys that request-time scope resolution
/// produces (see [`ene_connector::declaration::resolve_scope`]): the vault
/// itself never derives them. Candidate ids are every `plugins.list` name (a
/// plugin that declares its own name as its credential id, e.g. `anthropic`)
/// plus every `ai.providers` kind (a provider aliased under a different name
/// still feeds the credential id matching its kind, e.g.
/// `ai.providers.my-anthropic` with `kind: "anthropic"` feeds `anthropic`).
///
/// Resolution precedence per candidate id:
/// 1. `plugins.list.<id>.config.api_key` (plain string or `{"source": ...}`).
/// 2. `ai.providers.<alias>.api_key` for the first provider whose `kind`
///    equals the id (matched by kind, not alias name).
/// 3. The `{ID}_API_KEY` environment variable.
///
/// In addition, every id declared in `registry` gets an entry under its
/// resolved storage key (the plain id for shared declarations,
/// `<plugin>:<id>` for private ones), deduplicated against the candidate
/// path. A declared id resolves with its own precedence:
/// 1. `plugins.list.<plugin>.config.api_key` (the declaring plugin's config).
/// 2. `ai.providers.<alias>.api_key` matched by kind == the declared id.
/// 3. The declaration's `env_fallback`, else the `{ID}_API_KEY` environment
///    variable.
///
/// Persisted `OAuth2` credentials are preloaded from `persister` (the
/// durable home for flow-acquired tokens); a config-derived API key always
/// wins over a persisted entry for the same key. Stale entries whose plugin
/// was uninstalled are unreachable regardless — scope enforcement denies any
/// id the plugin no longer declares.
///
/// This mirrors the resolution contract the anthropic plugin previously
/// implemented in-process; the plugin-side copy is removed with the
/// credential client API.
#[cfg(any(unix, windows))]
fn build_credential_vault(
    config: &EneConfig,
    registry: &CredentialRegistry,
    persister: &dyn CredentialPersister,
    refresher: Option<Arc<dyn ene_connector::vault::TokenRefresher>>,
) -> CredentialVault {
    let plugin_config = config
        .get_section::<ene_plugin_host::PluginConfig>()
        .unwrap_or_default();
    let ai_config = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();

    let mut ids: Vec<String> = plugin_config.list.keys().cloned().collect();
    ids.extend(ai_config.providers.values().map(|def| def.kind.clone()));
    ids.sort();
    ids.dedup();

    let mut entries = Vec::new();
    let mut resolved: HashSet<String> = HashSet::new();
    for id in &ids {
        let Some(key) = resolve_credential_key(id, &plugin_config, &ai_config) else {
            continue;
        };
        entries.push(VaultEntry::new(
            id.clone(),
            CredentialStore::from_api_key(key),
        ));
        resolved.insert(id.clone());
    }

    for plugin in registry.plugins() {
        let declared = registry.declarations(&plugin);
        for decl in &declared {
            let ScopeDecision::Allowed { storage_key } =
                resolve_scope(&plugin, &declared, &decl.id)
            else {
                continue;
            };
            if resolved.contains(&storage_key) {
                continue;
            }
            let Some(key) = resolve_declared_credential_key(
                &plugin,
                &decl.id.to_string(),
                decl,
                &plugin_config,
                &ai_config,
            ) else {
                continue;
            };
            entries.push(VaultEntry::new(
                storage_key.clone(),
                CredentialStore::from_api_key(key),
            ));
            resolved.insert(storage_key);
        }
    }

    for (key, data) in persister.load() {
        let store = CredentialStore::from_exported(data);
        // Persisted entries are OAuth2 by construction, but guard the
        // variant so an on-disk anomaly cannot override a config API key.
        if store.is_oauth2() && !resolved.contains(&key) {
            entries.push(VaultEntry::new(key, store));
        }
    }

    let mut vault = CredentialVault::new(entries);
    for id in &ids {
        // Only ids the vault has nothing for are ever "missing", so the
        // operator sees which configuration path to fill in.
        if !resolved.contains(id) {
            vault.set_hint(id, credential_missing_hint(id, &ai_config));
        }
    }
    for plugin in registry.plugins() {
        let declared = registry.declarations(&plugin);
        for decl in &declared {
            let ScopeDecision::Allowed { storage_key } =
                resolve_scope(&plugin, &declared, &decl.id)
            else {
                continue;
            };
            if !resolved.contains(&storage_key) {
                vault.set_hint(
                    &storage_key,
                    credential_missing_hint(&decl.id.to_string(), &ai_config),
                );
            }
        }
    }
    if let Some(refresher) = refresher {
        vault = vault.with_refresher(refresher);
    }
    vault
}

/// Rebuilds the credential vault from the current config, the (now
/// populated) declaration registry, and the persisted OAuth tokens, then
/// swaps it into the passenger so connected clients are told which storage
/// keys changed. The OAuth flow manager holds the same vault snapshot for
/// flow completion and revocation, so it is swapped alongside the passenger.
pub(super) fn refresh_credential_vault(
    config: &EneConfig,
    registry: &Arc<CredentialRegistry>,
    passenger: &CredentialPassenger,
) {
    #[cfg(any(unix, windows))]
    {
        let persister: Arc<dyn CredentialPersister> = Arc::new(FileCredentialPersister::new(
            ene_config::paths::credential_file_path(),
        ));
        let refresher: Arc<dyn ene_connector::vault::TokenRefresher> = Arc::new(
            OAuthRefresher::new(Arc::clone(registry), Arc::clone(&persister)),
        );
        let vault = Arc::new(build_credential_vault(
            config,
            registry,
            &*persister,
            Some(refresher),
        ));
        passenger.replace_vault_and_broadcast(Arc::clone(&vault));
        if let Some(flow) = passenger.oauth_flow() {
            flow.swap_vault(vault);
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (config, registry, passenger);
    }
}

#[cfg(any(unix, windows))]
/// Non-secret setup guidance for a missing credential: the configuration
/// paths the runtime would consult, naming the concrete `ai.providers`
/// alias when one matches by kind.
fn credential_missing_hint(id: &str, ai_config: &ene_ai::AiConfig) -> String {
    let alias = ai_config
        .providers
        .iter()
        .find(|(_, def)| def.kind == id)
        .map(|(alias, _)| alias.as_str());
    let kind_path = match alias {
        Some(alias) => format!("ai.providers.{alias}.api_key (kind: {id})"),
        None => format!("ai.providers.<alias>.api_key with kind \"{id}\""),
    };
    format!(
        "set plugins.list.{id}.config.api_key, {kind_path}, or the {} environment variable",
        env_var_for_id(id)
    )
}

#[cfg(any(unix, windows))]
/// Resolves a credential key for `id` across the three configuration sources.
fn resolve_credential_key(
    id: &str,
    plugin_config: &ene_plugin_host::PluginConfig,
    ai_config: &ene_ai::AiConfig,
) -> Option<String> {
    // 1. plugins.list.<id>.config.api_key (anthropic contract).
    if let Some(entry) = plugin_config.list.get(id)
        && let Some(key) = entry
            .config
            .get("api_key")
            .and_then(|value| resolve_api_key_value(id, value))
    {
        return Some(key);
    }
    // 2. ai.providers.<alias>.api_key matched by kind: the alias name is
    // arbitrary, only the backend kind identifies the credential id.
    if let Some(def) = ai_config.providers.values().find(|def| def.kind == id)
        && let Some(key) = resolve_api_key_config(id, &def.api_key)
    {
        return Some(key);
    }
    // 3. {ID}_API_KEY environment fallback.
    std::env::var(env_var_for_id(id))
        .ok()
        .filter(|key| !key.trim().is_empty())
}

#[cfg(any(unix, windows))]
/// Resolves a key for a credential id declared by `plugin`, with the
/// declaration-specific precedence (see [`build_credential_vault`]): the
/// declaring plugin's own config first, then `ai.providers` matched by kind,
/// then an allowlisted declaration `env_fallback` or the conventional
/// `{ID}_API_KEY`.
fn resolve_declared_credential_key(
    plugin: &str,
    id: &str,
    decl: &CredentialDeclaration,
    plugin_config: &ene_plugin_host::PluginConfig,
    ai_config: &ene_ai::AiConfig,
) -> Option<String> {
    // 1. The declaring plugin's own config (the plugin-name key, not the id).
    if let Some(entry) = plugin_config.list.get(plugin)
        && let Some(key) = entry
            .config
            .get("api_key")
            .and_then(|value| resolve_api_key_value(plugin, value))
    {
        return Some(key);
    }
    // 2. ai.providers.<alias>.api_key matched by kind == the declared id.
    if let Some(def) = ai_config.providers.values().find(|def| def.kind == id)
        && let Some(key) = resolve_api_key_config(id, &def.api_key)
    {
        return Some(key);
    }
    // 3. The declaration's own env fallback, else the {ID}_API_KEY convention.
    let env_var = match &decl.kind {
        CredentialKind::ApiKey { env_fallback, .. } => {
            env_fallback.clone().unwrap_or_else(|| env_var_for_id(id))
        }
        CredentialKind::OAuth2 { .. } => env_var_for_id(id),
    };
    let conventional = env_var_for_id(id);
    let allowed = env_var == conventional
        || plugin_config
            .list
            .get(plugin)
            .is_some_and(|entry| entry.credential_env_allowlist.contains(&env_var));
    if !allowed {
        return None;
    }
    std::env::var(env_var)
        .ok()
        .filter(|key| !key.trim().is_empty())
}

#[cfg(any(unix, windows))]
/// Resolves a typed `ApiKeyConfig` to a key value, or `None` when the config
/// has no usable value.
fn resolve_api_key_config(id: &str, cfg: &ene_ai::ApiKeyConfig) -> Option<String> {
    if cfg.source == "inline" && !cfg.inline.trim().is_empty() {
        return Some(cfg.inline.trim().to_string());
    }
    if cfg.source == "env" {
        let var = if cfg.env.trim().is_empty() {
            env_var_for_id(id)
        } else {
            cfg.env.clone()
        };
        if let Ok(key) = std::env::var(&var)
            && !key.trim().is_empty()
        {
            return Some(key.trim().to_string());
        }
    }
    None
}

#[cfg(any(unix, windows))]
/// The environment variable name conventionally backing a credential id
/// (e.g. `anthropic` → `ANTHROPIC_API_KEY`).
fn env_var_for_id(id: &str) -> String {
    format!("{}_API_KEY", id.to_uppercase().replace('-', "_"))
}

#[cfg(any(unix, windows))]
/// Resolves a `plugins.list.<name>.config.api_key` value per the
/// `{"source": ...}` contract: a plain string is used directly;
/// `{"source": "inline", "inline": ...}` uses the inline value;
/// `{"source": "env", "env": "VAR"}` reads the named environment variable
/// (empty name → `{ID}_API_KEY`). Any other shape (including
/// `{"source": "auto"}`) resolves to `None` so the caller falls through to
/// the next source.
fn resolve_api_key_value(id: &str, value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(key) if !key.trim().is_empty() => Some(key.trim().to_string()),
        serde_json::Value::Object(obj) => {
            let source = obj
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("auto");
            match source {
                "inline" => obj
                    .get("inline")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .map(str::to_string),
                "env" => {
                    let var = obj
                        .get("env")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map_or_else(|| env_var_for_id(id), str::to_string);
                    std::env::var(var).ok().filter(|key| !key.is_empty())
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(any(unix, windows))]
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
    };
    DiagnosticEvent::ToolHealth {
        tool,
        status: status.to_string(),
        detail,
    }
}

pub(super) fn init_embedding(
    config: &EneConfig,
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
            let provider = ene_ai_local::create_local_provider(&local)?;
            Ok(Arc::from(provider))
        }
        ResolvedEmbedding::Cloud {
            base_url,
            api_key,
            model,
            dimensions,
            query_prefix,
        } => Ok(Arc::new(
            ene_ai::CloudEmbeddingProvider::new(
                &base_url,
                &api_key,
                &model,
                dimensions,
                query_prefix,
            )
            .with_retry_policy(ai_config.retry.to_policy()),
        )),
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

    if let Some(parent) = db_path.parent()
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

    #[test]
    fn stale_factory_diff_keeps_kinds_served_by_new_host() {
        let old_kinds = vec!["openai-compatible".to_string(), "stale-plugin".to_string()];
        let mut new_factories = HashMap::new();
        new_factories.insert(
            "openai-compatible".to_string(),
            Arc::new(ene_ai::OpenAiProviderFactory) as Arc<dyn ene_ai::LlmProviderFactory>,
        );

        assert_eq!(
            stale_llm_factory_names(&old_kinds, Some(&new_factories)),
            vec!["stale-plugin"]
        );
        assert_eq!(stale_llm_factory_names(&old_kinds, None), old_kinds);
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

    /// A persister over a non-existent temp file, so vault-builder tests
    /// seed only from config and registry declarations (never OAuth
    /// persistence).
    #[cfg(any(unix, windows))]
    fn vault_test_persister() -> Arc<dyn CredentialPersister> {
        Arc::new(FileCredentialPersister::new(
            std::env::temp_dir().join(format!("ene-vault-test-{}.json", std::process::id())),
        ))
    }

    /// Clean config for vault-builder tests: an empty plugin list (so the
    /// built-in `default_plugin_list()` entries cannot add stray candidate
    /// ids) and only the providers given as `(alias, kind)`, so no ambient
    /// `{ID}_API_KEY` environment variable can inject an entry.
    #[cfg(any(unix, windows))]
    fn vault_test_config(plugin_keys: &[(&str, &str)], providers: &[(&str, &str)]) -> EneConfig {
        let mut config = EneConfig::default();
        let mut plugins = ene_plugin_host::PluginConfig {
            enabled: true,
            list: HashMap::new(),
            ..Default::default()
        };
        for (name, key) in plugin_keys {
            plugins.list.insert(
                name.to_string(),
                ene_plugin_host::PluginEntry {
                    config: serde_json::json!({ "api_key": key }),
                    ..Default::default()
                },
            );
        }
        drop(config.set_section(&plugins));
        let mut ai = ene_ai::AiConfig {
            providers: std::collections::BTreeMap::default(),
            ..Default::default()
        };
        for (alias, kind) in providers {
            ai.providers.insert(
                alias.to_string(),
                ene_ai::AiProviderDef {
                    kind: kind.to_string(),
                    api_key: ene_ai::ApiKeyConfig {
                        source: "inline".to_string(),
                        inline: format!("sk-{alias}"),
                        env: String::new(),
                    },
                    ..Default::default()
                },
            );
        }
        drop(config.set_section(&ai));
        config
    }

    /// A private declaration must seed the `<plugin>:<id>` storage key that
    /// request-time scope resolution produces; the plain candidate id alone
    /// would leave private credentials permanently missing.
    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn vault_resolves_private_declaration_storage_key() {
        let config = vault_test_config(&[("anthropic", "sk-plugin")], &[]);
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "anthropic",
            Some(&serde_json::json!({
                "x-ene-credentials": [{
                    "id": "anthropic",
                    "kind": "api_key",
                    "shared": false
                }]
            })),
        );
        let vault = build_credential_vault(&config, &registry, &*vault_test_persister(), None);
        let store = vault
            .resolve("anthropic:anthropic")
            .await
            .expect("private declaration seeded under its storage key");
        assert_eq!(store.api_key(), Some("sk-plugin"));
    }

    /// A shared declaration and the provider-kind candidate path resolve to
    /// the same plain storage key; the vault must not create a duplicate
    /// entry for it.
    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn vault_deduplicates_shared_declaration_with_candidate() {
        let config = vault_test_config(&[], &[("my-anthropic", "anthropic")]);
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "custom",
            Some(&serde_json::json!({
                "x-ene-credentials": [{ "id": "anthropic", "kind": "api_key" }]
            })),
        );
        let vault = build_credential_vault(&config, &registry, &*vault_test_persister(), None);
        let store = vault
            .resolve("anthropic")
            .await
            .expect("shared declaration seeded");
        assert_eq!(store.api_key(), Some("sk-my-anthropic"));
        assert_eq!(vault.storage_keys(), vec!["anthropic".to_string()]);
    }

    /// The declaration's `env_fallback` wins over the conventional
    /// `{ID}_API_KEY` variable when both are set.
    #[cfg(any(unix, windows))]
    #[tokio::test]
    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "test-only set_var/remove_var on a unique variable name"
    )]
    async fn vault_prefers_declared_env_fallback_over_convention() {
        unsafe { std::env::set_var("ENE_VAULT_TEST_FALLBACK_VAR", "sk-fallback") };
        unsafe { std::env::set_var("FALLBACK_KEY_API_KEY", "sk-convention") };
        let mut config = vault_test_config(&[], &[]);
        let mut plugins = config
            .get_section::<ene_plugin_host::PluginConfig>()
            .expect("plugin config section");
        plugins
            .list
            .entry("custom".to_string())
            .or_default()
            .credential_env_allowlist
            .push("ENE_VAULT_TEST_FALLBACK_VAR".to_string());
        drop(config.set_section(&plugins));
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "custom",
            Some(&serde_json::json!({
                "x-ene-credentials": [{
                    "id": "fallback-key",
                    "kind": "api_key",
                    "env_fallback": "ENE_VAULT_TEST_FALLBACK_VAR"
                }]
            })),
        );
        let vault = build_credential_vault(&config, &registry, &*vault_test_persister(), None);
        unsafe { std::env::remove_var("ENE_VAULT_TEST_FALLBACK_VAR") };
        unsafe { std::env::remove_var("FALLBACK_KEY_API_KEY") };
        let store = vault
            .resolve("fallback-key")
            .await
            .expect("env_fallback seeded the entry");
        assert_eq!(store.api_key(), Some("sk-fallback"));
    }

    /// A declaration cannot turn an arbitrary host environment variable into
    /// a credential source without an explicit host allowlist entry.
    #[cfg(any(unix, windows))]
    #[tokio::test]
    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "test-only set_var/remove_var on a unique variable name"
    )]
    async fn vault_ignores_unallowlisted_declared_env_fallback() {
        unsafe { std::env::set_var("ENE_VAULT_TEST_UNALLOWLISTED_VAR", "sk-secret") };
        let config = vault_test_config(&[], &[]);
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "custom",
            Some(&serde_json::json!({
                "x-ene-credentials": [{
                    "id": "unallowlisted-key",
                    "kind": "api_key",
                    "env_fallback": "ENE_VAULT_TEST_UNALLOWLISTED_VAR"
                }]
            })),
        );
        let vault = build_credential_vault(&config, &registry, &*vault_test_persister(), None);
        unsafe { std::env::remove_var("ENE_VAULT_TEST_UNALLOWLISTED_VAR") };
        assert!(vault.resolve("unallowlisted-key").await.is_err());
    }

    /// Without a declared `env_fallback`, the conventional `{ID}_API_KEY`
    /// variable backs the credential.
    #[cfg(any(unix, windows))]
    #[tokio::test]
    #[expect(
        clippy::undocumented_unsafe_blocks,
        reason = "test-only set_var/remove_var on a unique variable name"
    )]
    async fn vault_falls_back_to_conventional_env_var() {
        unsafe { std::env::set_var("PLAIN_KEY_API_KEY", "sk-plain") };
        let config = vault_test_config(&[], &[]);
        let registry = CredentialRegistry::new();
        registry.register_from_schema(
            "custom",
            Some(&serde_json::json!({
                "x-ene-credentials": [{ "id": "plain-key", "kind": "api_key" }]
            })),
        );
        let vault = build_credential_vault(&config, &registry, &*vault_test_persister(), None);
        unsafe { std::env::remove_var("PLAIN_KEY_API_KEY") };
        let store = vault
            .resolve("plain-key")
            .await
            .expect("conventional env var seeded the entry");
        assert_eq!(store.api_key(), Some("sk-plain"));
    }

    #[test]
    fn plugin_config_blob_updates_clear_removed_enabled_entries() {
        let mut prev = ene_plugin_host::PluginConfig {
            list: HashMap::new(),
            ..Default::default()
        };
        prev.list.insert(
            "demo".to_string(),
            ene_plugin_host::PluginEntry {
                config: serde_json::json!({"setting": "old"}),
                ..Default::default()
            },
        );
        let next = ene_plugin_host::PluginConfig {
            list: HashMap::new(),
            ..Default::default()
        };

        let updates = plugin_config_blob_updates(&prev, &next);
        assert_eq!(updates.get("demo"), Some(&(None, None)));
    }
}
