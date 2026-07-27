//! `TurnActor` (formerly `EneActor`): the single-threaded actor that owns
//! turn-execution state (#271).
//!
//! ## What moved out (#271)
//!
//! - Read-only session queries and pending-candidate approval used to be
//!   `EneCommand` variants handled here even though they never touched
//!   `active_turn` / `stream_handle` / `turn_gate` — see
//!   [`crate::query::sessions::SessionQueryHandle`] and
//!   [`crate::query::candidates::MemoryCandidateHandle`], which now talk to
//!   `MemoryStore` directly.
//! - Screen-image vision summarization used to run the actual model call
//!   inside a `vision_tasks` `JoinSet` owned by this actor, with the raw RGB
//!   buffer riding through `EneCommand`. See [`crate::vision::VisionHandle`],
//!   which now performs that call itself; this actor only answers a small
//!   `PrepareVisionSummary` request (busy-check + lazy model handle) and a
//!   fire-and-forget `StashProactiveScreenImage`.
//!
//! ## What stayed (deliberately, see PR description)
//!
//! Turn execution (`Run` / `Cancel`), permission decisions, user-input
//! responses, snapshot, manual split/undo, tool calls, character-card swap,
//! feature/proactive settings updates, tool-index invalidation, `CCv3`
//! memory hash, and plugin host restart all still go through this single
//! actor. The issue's ideal design further splits config/control operations
//! (`SetCharacter`, `UpdateFeatureSettings`, plugin host restart) into a
//! separate `ControlPlane` actor from turn-execution-critical state
//! (`turn_gate`, `active_turn`, `stream_handle`, `undo_stack`). That further
//! split is deferred — see the PR body for why — since it does not
//! contribute to the head-of-line-blocking problem the issue is about
//! (config/control commands are already infrequent, low-latency, and never
//! block behind a `Run` turn any worse than `Run` itself already does).

use super::TurnGate;
use super::command::{DeferredToolTask, EneCommand, FeatureSettingsUpdate};
use super::event::{
    AudioChunk, EneEvent, EneStateSnapshot, EneStatus, LifecycleEvent, TerminalReason,
};
use crate::diagnostics::{DiagnosticEvent, MemoryQueryHandle, emit_diag};
use crate::error::EneRuntimeError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::{RequestId, TurnId};
use crate::vision::VisionPrepared;
use ene_ai::{AiTaskKind, LlmProviderRegistry, create_task_chat_provider};
use ene_config::EneConfig;
use ene_mind::CardName;
use ene_mind::commitments::CommitmentLedger;
use ene_mind::{
    CompressionLevel, CompressionTaskInput, HistoryEntry as MindHistoryEntry,
    compression_has_usable_summary,
};
use ene_mind::{ConversationSession, EneSessionError, SplitResult};
use ene_plugin_host::{CompositeToolRegistry, PluginHealthEvent, PluginHostError, ToolRegistry};
#[cfg(any(unix, windows))]
use ene_store::db_server::DbIpcServer;
use ene_tool_rag::{ToolRag, ToolRagConfig, ToolRagOptions};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Global monotonic counter used to generate unique DB IPC auth tokens.
/// Intentionally process-global: each `EneHandle::open` call increments
/// the counter so concurrent handles never share a token.
static DB_TOKEN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) struct TurnActor {
    cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
    event_tx: broadcast::Sender<EneEvent>,
    /// Lifecycle bus sender (#272): `StatusChanged` / `PendingCandidateAvailable`
    /// / `ToolBackgroundCompleted` — kept off the chat `event_tx` broadcast.
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    /// Audio channel sender (#272): bounded `mpsc`, cloned into each stream
    /// task's [`crate::streaming::StreamContext`] so TTS chunks never ride
    /// the chat broadcast bus.
    audio_tx: mpsc::Sender<AudioChunk>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
    turn_gate: Arc<TurnGate>,
    config: EneConfig,
    session: ConversationSession,
    registry: Arc<dyn ToolRegistry>,
    tool_rag: Option<Arc<ToolRag>>,
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<streaming::StreamOutcome>>,
    /// Shared with the running stream task; accumulates streamed assistant
    /// text deltas so a hard-aborted turn can still recover its partial
    /// response for interruption recording (#H5).
    stream_partial_text: Arc<parking_lot::Mutex<String>>,
    active_turn: Option<TurnId>,
    /// Cancellation token for the in-flight [`crate::vision::VisionHandle`]
    /// inference, if any. A fresh token is minted and handed out with each
    /// [`VisionPrepared`] reply; starting a new user turn cancels the
    /// current token and replaces it with a fresh one so a later vision call
    /// is not pre-cancelled. The actual inference call runs entirely outside
    /// this actor (#271) — this token is the only thread the actor still
    /// holds into an in-flight vision request, used solely to ask it to
    /// stop (it flows into `ene_infer::JobContext::should_stop` on the
    /// local llama.cpp worker).
    vision_cancel: CancellationToken,
    pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    /// Session-wide permission grants tracked for the permission center (#177).
    permission_scopes: Arc<Mutex<Vec<crate::streaming::PermissionScope>>>,
    /// Actor-native undo stack of mutating tool calls (#178).
    undo_stack: Arc<Mutex<crate::undo::UndoStack>>,
    context: ene_mind::ContextManager,
    call_tool_tasks: tokio::task::JoinSet<()>,
    classifier_tasks: tokio::task::JoinSet<()>,
    memory_writer_tasks: tokio::task::JoinSet<()>,
    /// In-flight tool-search jobs; reaped so panics are not lost (#236).
    search_tasks: tokio::task::JoinSet<()>,
    /// In-flight deferred (background) tool tasks (#196). Each task polls
    /// its owning tool until the task reaches a terminal state, then emits
    /// [`LifecycleEvent::ToolBackgroundCompleted`]. Reaped so panics are not lost.
    deferred_tool_tasks: tokio::task::JoinSet<()>,
    classifier_rx: mpsc::UnboundedReceiver<tokio::task::JoinHandle<()>>,
    memory_writer_rx:
        mpsc::UnboundedReceiver<tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>>,
    /// Receiver for deferred (background) tool tasks accepted by the stream (#196).
    deferred_tool_rx: mpsc::UnboundedReceiver<DeferredToolTask>,
    /// Sender for classifier `JoinHandles` from the stream task.
    classifier_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    /// Sender for deferred memory-writer `JoinHandles` from the stream task.
    memory_writer_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome>>,
    /// Sender for deferred (background) tool tasks accepted by the stream (#196).
    deferred_tool_tx: mpsc::UnboundedSender<DeferredToolTask>,
    /// Shared with the running stream task; first party to flip emits Terminal.
    terminal_emitted: Arc<AtomicBool>,
    /// Held so the broadcast channel retains buffered diagnostic events until the
    /// first subscriber attaches via [`crate::EneHandle::diagnostics().subscribe()`].
    _diag_rx: broadcast::Receiver<DiagnosticEvent>,
    /// Proactive speech scheduler state (#166).
    proactive: crate::proactive::ProactiveScheduler,
    /// In-flight decision result channel.
    proactive_decision_rx: Option<oneshot::Receiver<crate::proactive::ProactiveDecisionResult>>,
    /// Join handle for the in-flight decision task (aborted on user turn / shutdown).
    proactive_decision_handle: Option<tokio::task::JoinHandle<()>>,
    /// Local / cloud decision provider handles (lazy).
    proactive_llm: Option<ene_ai_local::ProactiveLlmHandles>,
    /// Origin of the active stream turn (for cancel Terminal).
    active_origin: crate::types::TurnOrigin,
    /// Provider health monitor for failover routing (#175).
    health_monitor: ene_ai::ProviderHealthMonitor,
    /// Maximum poll iterations for deferred (background) tool tasks (#196).
    /// Configurable via `ENE_TOOLS__DEFERRED_MAX_POLLS` env var (default: 600 = 60s).
    deferred_max_polls: u32,
    /// Resolved TTS provider for streaming audio synthesis (None when TTS is disabled).
    tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
    /// Plugin-contributed tool registries, re-merged when the tool registry is
    /// rebuilt after a Features update (#247).
    plugin_tool_registries: Vec<Arc<dyn ToolRegistry>>,
    /// Shared plugin host manager handle. Held by the actor so a Features
    /// update that changes the enabled plugin set can restart the host with
    /// the new configuration (E1). Shared with [`crate::EneHandle`] so shutdown
    /// tears down whichever host is currently live.
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    /// Shared handle to the plugin health → diagnostics bridge task. Kept in
    /// sync when the plugin host is restarted so shutdown aborts the live
    /// bridge rather than a stale one (#238).
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Current character-card name, shared with
    /// [`crate::query::candidates::MemoryCandidateHandle`] so pending-candidate
    /// queries can read it without a mailbox round-trip (#271). Kept in sync
    /// on every `SetCharacter`.
    card_name: Arc<parking_lot::Mutex<String>>,
}

impl TurnActor {
    /// Constructs a ready `TurnActor`. Called once from [`crate::EneHandle::open`].
    #[expect(
        clippy::too_many_arguments,
        reason = "single internal constructor call site (EneHandle::open); the #272 \
                  event-bus split added two channel senders (lifecycle_tx, audio_tx) \
                  pushing this from 15 to 17 — grouping into a config struct is a \
                  larger refactor out of scope here"
    )]
    pub(super) fn new(
        cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
        event_tx: broadcast::Sender<EneEvent>,
        lifecycle_tx: broadcast::Sender<LifecycleEvent>,
        audio_tx: mpsc::Sender<AudioChunk>,
        diag_tx: broadcast::Sender<DiagnosticEvent>,
        diag_rx: broadcast::Receiver<DiagnosticEvent>,
        turn_gate: Arc<TurnGate>,
        config: EneConfig,
        session: ConversationSession,
        registry: Arc<dyn ToolRegistry>,
        tool_rag: Option<Arc<ToolRag>>,
        health_monitor: ene_ai::ProviderHealthMonitor,
        tts_provider: Option<Arc<dyn ene_ai::TtsProvider>>,
        plugin_tool_registries: Vec<Arc<dyn ToolRegistry>>,
        plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
        health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        card_name: Arc<parking_lot::Mutex<String>>,
    ) -> Self {
        let (classifier_tx, classifier_rx) = mpsc::unbounded_channel();
        let (memory_writer_tx, memory_writer_rx) = mpsc::unbounded_channel();
        let (deferred_tool_tx, deferred_tool_rx) = mpsc::unbounded_channel();
        Self {
            cmd_rx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            diag_tx,
            turn_gate,
            config,
            session,
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
            search_tasks: tokio::task::JoinSet::new(),
            deferred_tool_tasks: tokio::task::JoinSet::new(),
            classifier_rx,
            memory_writer_rx,
            deferred_tool_rx,
            terminal_emitted: Arc::new(AtomicBool::new(false)),
            classifier_tx,
            memory_writer_tx,
            deferred_tool_tx,
            _diag_rx: diag_rx,
            proactive: crate::proactive::ProactiveScheduler::default(),
            proactive_decision_rx: None,
            proactive_decision_handle: None,
            proactive_llm: None,
            active_origin: crate::types::TurnOrigin::User,
            health_monitor,
            deferred_max_polls: std::env::var("ENE_TOOLS__DEFERRED_MAX_POLLS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(600),
            tts_provider,
            plugin_tool_registries,
            plugin_host,
            health_bridge_handle,
            card_name,
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

    pub(super) async fn run(mut self) {
        loop {
            // Reap completed background tasks so the JoinSets
            // do not grow without bound. Call-tool volume is
            // bounded by interactive `EneCommand::CallTool` rate.
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

            // Drain classifier JoinHandles sent from the stream.
            while let Ok(handle) = self.classifier_rx.try_recv() {
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

            // Drain deferred memory-writer JoinHandles sent from the stream.
            while let Ok(handle) = self.memory_writer_rx.try_recv() {
                let diag_tx = self.diag_tx.clone();
                let lifecycle_tx = self.lifecycle_tx.clone();
                let store = self.session.memory.memory_store.clone();
                self.memory_writer_tasks.spawn(async move {
                    match handle.await {
                        Ok(ene_mind::MemoryWriteOutcome::Ok {
                            deferred_candidates,
                        }) => {
                            if deferred_candidates > 0 {
                                drop(lifecycle_tx.send(
                                    LifecycleEvent::PendingCandidateAvailable {
                                        count: deferred_candidates,
                                    },
                                ));
                            }
                        }
                        Ok(ene_mind::MemoryWriteOutcome::Failed {
                            message,
                            pending_id,
                            permanent,
                            character_id,
                        }) => {
                            let (pending_count, permanent_count) = if let Some(store) =
                                store.as_ref()
                            {
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
                });
            }

            // Drain deferred tool tasks accepted by the stream (#196).
            // Spawn a polling task for each that awaits completion.
            while let Ok(task) = self.deferred_tool_rx.try_recv() {
                let registry = Arc::clone(&self.registry);
                let lifecycle_tx = self.lifecycle_tx.clone();
                let max_polls = self.deferred_max_polls;
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
                        // The stream task has finished (or the
                        // sender was dropped).
                        if let Ok(outcome) = res {
                            self.session = outcome.session;
                            if self.active_origin == crate::types::TurnOrigin::Proactive
                                && outcome.terminal == TerminalReason::Done
                            {
                                self.proactive.on_proactive_completed();
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

        // Clean up any running stream
        if let Some(handle) = self.stream_handle.take() {
            handle.abort();
        }
        if let Some(handles) = self.proactive_llm.take() {
            handles.shutdown().await;
        }
        // Abort any in-flight direct CallTool, classifier, and memory-writer tasks,
        // and clear any pending prompt oneshot senders.
        self.classifier_tasks.abort_all();
        self.memory_writer_tasks.abort_all();
        self.call_tool_tasks.abort_all();
        self.search_tasks.abort_all();
        self.drain_pending().await;
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

    /// Restarts the plugin host to apply a changed enabled-plugin set (E1).
    ///
    /// Shuts down the live host (stopping disabled plugins), spawns fresh DB
    /// IPC servers for the newly-detected plugin set, starts a new host with
    /// the updated configuration, re-registers any plugin-provided LLM
    /// factories, re-bridges health events into diagnostics, and rebuilds the
    /// tool registry from the new host.
    ///
    /// Remaining limitations:
    /// - Plugin-provided LLM factories are re-registered into the global
    ///   [`LlmProviderRegistry`], but factories from the previous host are not
    ///   unregistered (the registry has no deregistration API); a re-registered
    ///   kind simply replaces its entry.
    /// - DB IPC servers spawned for the previous host are not torn down; the
    ///   new set replaces them on the same per-plugin socket paths (the stale
    ///   socket file is removed before re-binding), so at most one live server
    ///   serves each plugin.
    async fn reconfigure_plugin_host(&mut self) {
        // Stop the previous host (and its health bridge) first.
        {
            let mut guard = self.plugin_host.lock().await;
            if let Some(mut host) = guard.take() {
                host.shutdown().await;
            }
            drop(guard);

            let mut bridge = self.health_bridge_handle.lock().await;
            if let Some(handle) = bridge.take() {
                handle.abort();
            }
        }

        // Spawn DB IPC servers for the (possibly changed) plugin set.
        let db_tokens =
            match spawn_db_ipc_servers(&self.config, self.session.memory.memory_store.as_ref()) {
                Ok(tokens) => tokens,
                Err(e) => {
                    tracing::warn!(
                        component = "TurnActor",
                        error = %e,
                        "Failed to spawn DB IPC servers during plugin reconfiguration; \
                         continuing without plugin DB access"
                    );
                    HashMap::new()
                }
            };

        // Start the new host with the updated configuration.
        let mut new_host =
            match ene_plugin_host::PluginHostManager::start(&self.config, db_tokens).await {
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

        // Re-register plugin-provided LLM factories (replaces prior entries).
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

        // Re-bridge health events into diagnostics with a fresh task.
        let mut bridge_handle: Option<tokio::task::JoinHandle<()>> = None;
        if let Some(host) = new_host.as_mut()
            && let Some(mut health_rx) = host.take_health_receiver()
        {
            let diag_tx = self.diag_tx.clone();
            bridge_handle = Some(tokio::spawn(async move {
                while let Some(event) = health_rx.recv().await {
                    emit_diag(&diag_tx, plugin_health_event_to_diag(event));
                }
            }));
        }

        // Rebuild the tool registry from the new host's registries.
        let registries = new_host
            .as_ref()
            .map_or_else(Vec::new, |h| h.tool_registries().to_vec());
        let registry_count = registries.len();
        match CompositeToolRegistry::try_new(registries.clone()) {
            Ok(composite) => {
                self.registry = Arc::new(composite);
                self.plugin_tool_registries = registries;
                tracing::info!(
                    component = "TurnActor",
                    tool_registries = registry_count,
                    "Plugin host reconfigured and tool registry rebuilt after Features update"
                );
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
        *self.plugin_host.lock().await = new_host;
        *self.health_bridge_handle.lock().await = bridge_handle;
    }

    async fn ensure_proactive_llm(&mut self) -> Result<(), crate::public_api::PublicApiError> {
        if self.proactive_llm.is_some() {
            return Ok(());
        }
        let ai_cfg = self.config.get_section::<ene_ai::AiConfig>().map_err(|e| {
            crate::public_api::PublicApiError::Internal {
                message: format!("AI config unavailable: {e}"),
            }
        })?;
        match ene_ai_local::build_proactive_llm_handles(&ai_cfg).await {
            Ok(handles) => {
                tracing::info!(
                    component = "Proactive",
                    decision_backend = ?handles.decision_kind,
                    vision = handles
                        .local()
                        .is_some_and(|l| l.capabilities().contains(ene_ai::Capability::Vision)),
                    "Proactive decision provider ready"
                );
                self.proactive_llm = Some(handles);
                Ok(())
            }
            Err(e) => Err(crate::public_api::PublicApiError::Internal {
                message: format!("Failed to start proactive decision provider: {e}"),
            }),
        }
    }

    /// Handles [`EneCommand::PrepareVisionSummary`] (#271): the busy-check
    /// and lazy local-model init the legacy inline `SummarizeScreenImage`
    /// handler used to do, minus the raw RGB buffer and the actual
    /// (expensive) model call — both of which now live entirely in
    /// [`crate::vision::VisionHandle`], outside this actor.
    async fn prepare_vision_summary(
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
                |m| m.emotion.classifier_language.clone(),
            );

        if let Err(e) = self.ensure_proactive_llm().await {
            drop(reply.send(Err(e)));
            return;
        }
        let Some(handles) = self.proactive_llm.as_ref() else {
            drop(reply.send(Err(PublicApiError::Internal {
                message: "proactive LLM handles missing after ensure".to_string(),
            })));
            return;
        };
        let Some(local) = handles.local().cloned() else {
            drop(reply.send(Err(PublicApiError::Internal {
                message: format!(
                    "local proactive model is not available (decision_backend={:?})",
                    handles.decision_kind
                ),
            })));
            return;
        };
        if !local.capabilities().contains(ene_ai::Capability::Vision) {
            drop(reply.send(Err(PublicApiError::Internal {
                message: "local model has no vision mmproj loaded".to_string(),
            })));
            return;
        }

        let prompts = ene_config::PromptLibrary::load(&prompt_language);
        let system = prompts.proactive().screen_summary_system.trim().to_string();
        let user = prompts.proactive().render_screen_summary_user(&app_label);
        // Mint a fresh cancel token for this request; a new user turn
        // cancels and replaces `self.vision_cancel` (see `EneCommand::Run`),
        // so an older, already-handed-out token being left cancelled from a
        // prior turn can never pre-cancel this new request.
        self.vision_cancel = CancellationToken::new();
        let cancel = self.vision_cancel.clone();
        drop(reply.send(Ok(VisionPrepared {
            local,
            system,
            user,
            cancel,
        })));
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

        if let Err(e) = self.ensure_proactive_llm().await {
            tracing::warn!(component = "Proactive", error = %e, "Failed to start proactive decision provider");
            return;
        }

        tracing::info!(
            component = "Proactive",
            interval_seconds = mind.proactive.interval_seconds,
            min_idle_seconds = mind.proactive.min_idle_seconds,
            "Proactive decision started"
        );

        let decision_provider = self.proactive_llm.as_ref().map(|h| Arc::clone(&h.decision));
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
        let prompt_language = mind.emotion.classifier_language.clone();
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
            &mind.emotion.classifier_language,
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

    async fn handle_command(&mut self, cmd: EneCommand) -> bool {
        match cmd {
            EneCommand::Run { input, turn } => {
                // Single-flight: Busy is enforced on the handle via TurnGate.
                // Never abort an in-flight turn here.
                if self.stream_handle.is_some() {
                    tracing::warn!(
                        component = "TurnActor",
                        "Run received while stream active; turn gate should have returned Busy"
                    );
                    return true;
                }
                // Discard any in-flight proactive decision and cancel any
                // in-flight vision summarization (#271 moved the inference
                // itself off this actor; this token is the only handle back
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
                            }
                        }
                    }
                }
                let _ = self.stream_session_rx.take();

                // Fallback: if the stream task was hard-aborted before it could
                // record the interruption, capture the partial response here (#206).
                // Read from the shared partial-text buffer that the stream task
                // updates live, since the session's display buffer is a pre-stream
                // snapshot that is empty after finalize (#H5).
                let partial = self.stream_partial_text.lock().clone();
                if !self.session.has_pending_interruption() && !partial.trim().is_empty() {
                    let spoken_chars = partial.chars().count();
                    self.session
                        .mark_interrupted(&turn.to_string(), &partial, spoken_chars);
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
                self.classifier_tasks.abort_all();
                self.memory_writer_tasks.abort_all();
                self.call_tool_tasks.abort_all();
                self.abort_proactive_decision();
                if let Some(handles) = self.proactive_llm.take() {
                    handles.shutdown().await;
                }
                self.drain_pending().await;
                false
            }
            EneCommand::SetCharacter { card, reply } => {
                self.session.set_card(&card);
                *self.card_name.lock() = self.session.card_name().to_string();
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

                drop(self.config.set_section(&mind));
                drop(self.config.set_section(&store));
                drop(self.config.set_section(&plugins));
                drop(self.config.set_section(&rag));
                self.abort_proactive_decision();

                if plugins_changed {
                    // The enabled plugin set changed: restart the plugin host
                    // so newly-enabled plugins are spawned and disabled ones
                    // are stopped, then rebuild the tool registry from the new
                    // host (E1). The previous behavior rebuilt a static list of
                    // already-live registries, which was effectively a no-op.
                    self.reconfigure_plugin_host().await;
                }
                true
            }
            EneCommand::PrepareVisionSummary { app_label, reply } => {
                self.prepare_vision_summary(app_label, reply).await;
                true
            }
            EneCommand::StashProactiveScreenImage { data_uri } => {
                self.proactive.last_screen_image_data_uri = data_uri;
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
            EneCommand::ManualSplit { reply } => {
                let result = self.handle_manual_split().await;
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
                    memory: MemoryQueryHandle::new(
                        self.session.memory.memory_store.clone(),
                        self.session.memory.embedding_provider.clone(),
                        self.config
                            .get_section::<ene_mind::MindConfig>()
                            .unwrap_or_default()
                            .memory,
                    ),
                    current_turn_count: self.session.current_turn_count() as u32,
                    session_started_at: self.session.session_started_at(),
                };
                drop(reply.send(snapshot));
                true
            }
            EneCommand::ListTools { reply } => {
                let mut tools = self.registry.list_tools();
                tools.push(crate::streaming::search_tools_spec());
                drop(reply.send(tools));
                true
            }
            EneCommand::SearchTools { query, reply } => {
                let registry = self.registry.clone();
                let tool_rag = self.tool_rag.clone();
                self.search_tasks.spawn(async move {
                    let result = if let Some(rag) = tool_rag {
                        let all_tools = registry.list_tools();
                        let profiles = registry.list_rag_profiles();
                        if let Err(e) = rag.ensure_index(&all_tools, &profiles).await {
                            tracing::warn!(component = "ToolRag", error = %e, "ensure_index failed");
                        }
                        rag.select(&query).await
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
                    drop(reply.send(result));
                });
                true
            }
            EneCommand::CallTool {
                name,
                arguments,
                turn,
                reply,
            } => {
                let registry = self.registry.clone();
                let tool_rag = self.tool_rag.clone();
                let session_id = self.session.memory.session_id.to_string();
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
                panic!("induced panic after mutating shared actor state (#268 regression test)");
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
            self.session.record_user_input();
            self.session.add_user_message(&user_input);
            self.check_and_perform_split(&user_input).await;
        }

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
        let tts_provider = self.tts_provider.clone();
        // Reset the shared partial-text buffer for this turn and hand a clone
        // to the stream task so a hard-abort can recover streamed text (#H5).
        self.stream_partial_text.lock().clear();
        let partial_text = Arc::clone(&self.stream_partial_text);
        self.active_origin = origin;

        drop(self.event_tx.send(EneEvent::TurnStarted {
            turn: turn_for_stream.clone(),
            origin,
        }));

        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

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
                        tts_provider,
                        partial_text,
                    })
                    .await
                }))
                .await
                {
                    Ok(outcome) => outcome,
                    Err(e) => {
                        // The stream task panicked. `EneActor::run_command_isolated`
                        // (#268) already contains the actor's own panic isolation,
                        // but that only protects the command loop from panics in
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
        // healthy one (#175). Probes send no user data.
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

    // ── Undo management (#178) ──

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

    async fn handle_manual_split(&mut self) -> Result<SplitResult, EneRuntimeError> {
        if self.session.history().is_empty() {
            return Err(EneRuntimeError::from(EneSessionError::SplitNotNeeded));
        }
        self.handle_manual_compression().await
    }

    async fn handle_manual_compression(&mut self) -> Result<SplitResult, EneRuntimeError> {
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
            config: mind.context.clone(),
        };
        let result = ene_mind::ContextManager::execute_manual(store, provider, input).await?;
        if compression_has_usable_summary(&result) {
            self.trim_history_after_compression();
        }
        Ok(SplitResult {
            reason: ene_mind::SplitReason::Manual,
            summary: result.summary,
            key_facts: vec![],
            new_session_id: self.session.memory.session_id.clone(),
            snapshot_len: self.session.history().len(),
        })
    }

    async fn check_and_perform_split(&mut self, _user_input: &str) {
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
        let Ok(provider) = self.create_provider().await else {
            return;
        };
        let turn_count = self.session.current_turn_count();
        let history = self.session.history().to_vec();
        self.context.check_and_trigger(
            &mind.context,
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
                    self.trim_history_after_compression();
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
        let history_len = self.session.history().len();
        if history_len > recent_cap {
            self.session.trim_history_keep_last(recent_cap);
        }
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
            mind.context,
        );
    }

    fn mind_history_entries(&self) -> Vec<MindHistoryEntry> {
        self.session.history().to_vec()
    }
}

/// Runs a future to completion, catching any panic and surfacing it as a
/// [`DiagnosticEvent::ActorPanic`] instead of unwinding the caller (#236).
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
/// surfacing panics through structured diagnostics (#236).
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

/// Polls a deferred (background) tool task until it reaches a terminal state (#196).
///
/// Emits [`LifecycleEvent::ToolBackgroundCompleted`] on the lifecycle bus
/// (not the chat bus) when the task completes, fails, or is cancelled — it
/// fires asynchronously after the originating turn has already completed
/// (#272). Runs as a background task in the actor's `deferred_tool_tasks`
/// `JoinSet`.
///
/// `max_polls` controls how many poll iterations (at 100ms each) before the task
/// is considered timed out. Override via the `ENE_TOOLS__DEFERRED_MAX_POLLS` env var
/// (default: 600 = 60s).
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

// ── Factory / init helpers (moved from runtime.rs) ──

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

/// Spawns per-tool DB IPC servers for tool plugins that need database access.
///
/// Returns a map of plugin name → auth token. The tokens are passed to
/// [`ene_plugin_host::PluginHostManager::start`] which hands them to the
/// tool binaries via `SandboxConfigData::db_auth_token`.
///
/// Token generation is driven by the **detected plugin set** (the binaries
/// actually discovered on disk) rather than by `plugins.list` config keys.
/// The host manager discovers plugins by scanning for `ene-plugin-{name}`
/// binaries and only consults config to skip explicitly-disabled names, so
/// keying tokens off config would orphan DB servers (config key with no
/// matching binary) or starve plugins of tokens (binary with no config
/// entry). Mirroring the manager's discovery here keeps the two sets aligned.
pub(super) fn spawn_db_ipc_servers(
    config: &EneConfig,
    memory_store: Option<&Arc<ene_store::MemoryStore>>,
) -> Result<HashMap<String, String>, EneRuntimeError> {
    let mut db_tokens = HashMap::new();
    let Some(store) = memory_store else {
        return Ok(db_tokens);
    };

    #[cfg(any(unix, windows))]
    {
        let plugin_config = config
            .get_section::<ene_plugin_host::PluginConfig>()
            .unwrap_or_default();

        // When the plugin system is disabled the host manager spawns nothing,
        // so no DB servers are needed; spawning them anyway would orphan them.
        if !plugin_config.enabled {
            return Ok(db_tokens);
        }

        let db = store.connection().clone();

        let socket_dir = ene_config::paths::tool_socket_dir();
        std::fs::create_dir_all(&socket_dir).map_err(|e| {
            EneRuntimeError::Tool(PluginHostError::ExecutionFailed {
                message: format!("Failed to create socket dir: {e}"),
            })
        })?;

        for name in discover_plugin_names() {
            // Skip plugins explicitly disabled in configuration, mirroring
            // `PluginHostManager::start`'s enable filter (a discovered binary
            // with no config entry is enabled by default).
            if let Some(entry) = plugin_config.list.get(&name)
                && !entry.enable
            {
                continue;
            }

            let tool_name = name.clone();
            let prefix = format!("{name}_");
            let socket_path = {
                #[cfg(unix)]
                {
                    socket_dir.join(format!("ene-db-{name}.sock"))
                }
                #[cfg(windows)]
                {
                    std::path::PathBuf::from(format!(r"\\.\pipe\ene-db-{}", name))
                }
            };

            // Generate a 128-bit pre-shared token for this tool's
            // DB IPC connection. We use a 256-bit keystream from
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

            let server = DbIpcServer::new(
                db.clone(),
                socket_path,
                tool_name.clone(),
                prefix,
                auth_token,
            );

            tokio::spawn(async move {
                if let Err(e) = server.run().await {
                    tracing::error!(tool = %tool_name, error = %e, "DB IPC server error");
                }
            });
        }
    }

    Ok(db_tokens)
}

/// Discovers plugin binary names by scanning the builtin and user plugin
/// directories for executables following the `ene-plugin-{name}` convention.
///
/// This intentionally mirrors `ene_plugin_host::manager::discover_plugins`
/// (which is private to that crate) so that DB token generation keys off the
/// exact same set of plugins the host manager will actually spawn. Keeping the
/// two discovery routines in lockstep is what prevents config-key ↔ binary-name
/// mismatches from orphaning DB servers or starving plugins of tokens.
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
            // Strip the exe suffix before matching the plugin prefix.
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

/// Builds the active composite tool registry from plugin-contributed registries.
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
/// with a stable English status contract (#238).
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
        PluginHealthEvent::Disabled { plugin } => (
            plugin,
            "disabled",
            Some("restart budget exhausted".to_string()),
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
    session: &ConversationSession,
) -> Result<Option<Arc<ToolRag>>, EneRuntimeError> {
    let rag_config = config.get_section::<ToolRagConfig>()?;

    if !rag_config.enabled {
        return Ok(None);
    }

    let store = session.memory.memory_store.clone();
    let opts = ToolRagOptions::from_config(rag_config)?;
    Ok(Some(Arc::new(ToolRag::new(embedder.clone(), store, opts))))
}
