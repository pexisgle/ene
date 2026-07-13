#[cfg(any(unix, windows))]
use crate::db_server::DbIpcServer;
use crate::diagnostics::{DiagnosticEvent, MemoryQueryHandle};
use crate::error::EneRuntimeError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::{CancelError, RequestId, RunError, TurnId};
use chrono::{DateTime, Utc};
use ene_ai::LlmProviderRegistry;
use ene_config::CharacterCardV3;
use ene_config::EneConfig;
use ene_mind::{CardName, SessionId};
use ene_mind::{
    CompressionLevel, CompressionTaskInput, HistoryEntry as MindHistoryEntry,
    MIN_MESSAGES_TO_COMPRESS, PendingCompressionTask, compression_has_usable_summary,
    evaluate_compression_trigger, execute_compression, maybe_roll_up_chapter,
    poll_compression_result, spawn_compression_task,
};
use ene_mind::{ConversationSession, EneSessionError, SplitResult};
use ene_tool_host::{ToolHostManager, ToolRegistry};
use ene_tool_proto::ToolSpec;
use std::collections::HashMap;
static DB_TOKEN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Commands sent to the actor from consumers (UI/CLI).
///
/// Fire-and-forget variants are sent via internal channels.
/// Oneshot variants carry a reply channel for result confirmation.
pub enum EneCommand {
    /// Start an AI completion for the given user prompt.
    Run {
        /// The raw user input to send to the LLM.
        input: String,
        /// Turn id allocated by the handle.
        turn: TurnId,
    },
    /// Cancel a specific in-flight turn.
    Cancel {
        /// Turn to cancel.
        turn: TurnId,
    },
    /// Shut down the actor and clean up background tasks.
    Shutdown,
    /// Submit a permission decision for a pending destructive operation.
    PermissionDecision {
        /// The `request_id` from a prior `PermissionRequired` event.
        request_id: RequestId,
        /// The user's decision.
        decision: PermissionDecision,
    },
    /// Submit a user-input response for a pending interactive tool.
    UserInputResponse {
        /// The `request_id` from a prior `UserInputRequired` event.
        request_id: RequestId,
        /// The user's response (selected option, free-text, or cancel).
        response: UserInputResponse,
    },
    /// Request a read-only snapshot of the current actor state (for CLI queries).
    GetSnapshot {
        /// Reply channel for the snapshot.
        reply: oneshot::Sender<EneStateSnapshot>,
    },
    /// Manually trigger a session split for the current conversation.
    ManualSplit {
        /// Result channel carrying the split result or an error.
        reply: oneshot::Sender<Result<SplitResult, EneRuntimeError>>,
    },
    /// List all tools in the active tool registry.
    ListTools {
        /// Reply channel for the tools.
        reply: oneshot::Sender<Vec<ToolSpec>>,
    },
    /// Call a tool by name with JSON-encoded arguments.
    CallTool {
        /// The tool name.
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
        /// Reply channel.
        reply: oneshot::Sender<Result<String, EneRuntimeError>>,
    },
    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    InvalidateToolIndex,
    /// Persist the CCv3 character-memory content hash after startup warmup.
    SetCcv3MemoryHash {
        /// Combined lorebook + style content hash.
        hash: u64,
        /// Confirmation channel.
        reply: oneshot::Sender<()>,
    },
    /// Replace the loaded character card.
    SetCharacter {
        /// New character card.
        card: Box<CharacterCardV3>,
        /// Confirmation channel.
        reply: oneshot::Sender<Result<(), EneRuntimeError>>,
    },
}

/// Events emitted from the actor to all consumers via broadcast channel.
///
/// Consumers (CLI, Bevy systems, logging) receive these through
/// [`EneHandle::subscribe`] which returns an [`EneEventReceiver`].
#[derive(Debug, Clone)]
pub enum EneEvent {
    /// A chunk of generated text from the LLM (markers stripped).
    TextDelta {
        /// Active turn.
        turn: TurnId,
        /// The raw text delta.
        delta: String,
    },
    /// Presentation cues (expression / emote) for the active turn.
    Performance {
        /// Active turn.
        turn: TurnId,
        /// Cue list (usually one expression).
        cues: Vec<ene_mind::PerformanceCue>,
        /// How the cues were chosen.
        source: ene_mind::CueSource,
    },
    /// A tool call has been requested by the LLM.
    ToolCallStart {
        /// Active turn.
        turn: TurnId,
        /// The tool name (e.g. "fs.write").
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// A tool call has completed with its result.
    ToolCallResult {
        /// Active turn.
        turn: TurnId,
        /// The tool name.
        name: String,
        /// The tool's output as a string.
        result: String,
    },
    /// A destructive operation requires user approval before execution.
    PermissionRequired {
        /// Active turn.
        turn: TurnId,
        /// Unique identifier for this permission request.
        request_id: RequestId,
        /// The category of operation (e.g. "write", "delete").
        action: String,
        /// The target resource path.
        target: String,
        /// Human-readable description of what will be done.
        description: String,
    },
    /// An interactive tool needs user input (e.g. a clarifying question).
    UserInputRequired {
        /// Active turn.
        turn: TurnId,
        /// Unique identifier for this input request.
        request_id: RequestId,
        /// The prompt describing the question, options, and free-text allowance.
        prompt: ene_tool_proto::UserInputPrompt,
    },
    /// Thin signal that rolling context compression completed for this turn.
    ContextCompressed {
        /// Active turn.
        turn: TurnId,
        /// Compression level label (e.g. "scene").
        level: String,
    },
    /// Terminal event for a run: emitted exactly once after after_turn completes.
    Terminal {
        /// Active turn.
        turn: TurnId,
        /// Why the run terminated.
        reason: TerminalReason,
    },
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
}

/// Reason a single run terminated.
///
/// Used in [`EneEvent::Terminal`]. Exactly one of these is emitted per
/// `EneCommand::Run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalReason {
    /// The LLM stream completed normally (no more tool calls and the
    /// provider finished the response).
    Done,
    /// The run terminated due to an error.
    Failed {
        /// Human-readable error description.
        message: String,
    },
    /// The run was cancelled by the user via `EneCommand::Cancel`.
    Cancelled,
}

/// A snapshot of the current actor state for read-only queries.
#[derive(Clone)]
pub struct EneStateSnapshot {
    /// The loaded character card, if any.
    pub character_card: Option<CharacterCardV3>,
    /// Conversation history (`ene_mind::HistoryEntry`).
    pub history: Vec<ene_mind::HistoryEntry>,
    /// A copy of the current configuration.
    pub config: EneConfig,
    /// Current session ID.
    pub session_id: SessionId,
    /// Character card name.
    pub card_name: CardName,
    /// Memory query handle (enabled only if memory is configured).
    /// Prefer [`crate::EneDiagnostics::memory`] for new code.
    pub memory: MemoryQueryHandle,
    /// Current conversation turn count.
    pub current_turn_count: u32,
    /// When the session started (UTC).
    pub session_started_at: DateTime<Utc>,
}

/// Current status of the actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EneStatus {
    /// Not currently processing anything.
    Idle,
    /// An AI stream is running.
    Running,
    /// An error state (non-fatal).
    Error,
}

/// Error returned when a command is sent to an actor that is no longer running.
#[derive(Debug, thiserror::Error)]
#[error("Actor is no longer running")]
pub struct ActorDeadError;

/// Error returned by [`EneHandle::shutdown`] when the actor's drain
/// takes longer than the supplied timeout.
#[derive(Debug, thiserror::Error)]
#[error("Actor did not shut down within {0:?}")]
pub struct ShutdownTimeout(pub std::time::Duration);

/// Event receiver handle obtained from [`EneHandle::subscribe`].
///
/// Wraps the broadcast receiver and provides a ergonomic interface for
/// consuming events from the actor.
pub struct EneEventReceiver(broadcast::Receiver<EneEvent>);

impl std::fmt::Debug for EneEventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EneEventReceiver").finish()
    }
}

impl EneEventReceiver {
    /// Non-blocking poll of the event stream.
    pub fn try_recv(&mut self) -> Result<EneEvent, broadcast::error::TryRecvError> {
        self.0.try_recv()
    }

    /// Async receive, waiting for the next event.
    pub async fn recv(&mut self) -> Result<EneEvent, broadcast::error::RecvError> {
        self.0.recv().await
    }
}

/// Shared single-flight turn gate between handle and actor.
struct TurnGate {
    busy: AtomicBool,
    active: std::sync::Mutex<Option<TurnId>>,
}

impl TurnGate {
    fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
            active: std::sync::Mutex::new(None),
        }
    }

    fn try_begin(&self, turn: &TurnId) -> bool {
        if self
            .busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        if let Ok(mut guard) = self.active.lock() {
            *guard = Some(turn.clone());
        }
        true
    }

    fn end(&self) {
        if let Ok(mut guard) = self.active.lock() {
            *guard = None;
        }
        self.busy.store(false, std::sync::atomic::Ordering::Release);
    }

    fn matches(&self, turn: &TurnId) -> bool {
        self.active
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .is_some_and(|active| active == *turn)
    }
}

/// Thread-safe handle to the ready actor.
///
/// Constructed only via [`EneHandle::open`], which initializes provider, store,
/// tools, mind session, and warmup before returning. When the last clone is
/// dropped the underlying `mpsc` channel closes and the actor exits.
pub struct EneHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    event_tx: broadcast::Sender<EneEvent>,
    diag_tx: broadcast::Sender<crate::diagnostics::DiagnosticEvent>,
    diagnostics: crate::diagnostics::EneDiagnostics,
    turn_gate: Arc<TurnGate>,
    /// `JoinHandle` for the actor task. Used by [`EneHandle::shutdown`]
    /// to await the actor's drain after sending `EneCommand::Shutdown`.
    actor_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl std::fmt::Debug for EneHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EneHandle").finish()
    }
}

impl Clone for EneHandle {
    fn clone(&self) -> Self {
        Self {
            cmd_tx: Arc::clone(&self.cmd_tx),
            event_tx: self.event_tx.clone(),
            diag_tx: self.diag_tx.clone(),
            diagnostics: crate::diagnostics::EneDiagnostics {
                cmd_tx: Arc::clone(&self.cmd_tx),
                diag_tx: self.diag_tx.clone(),
                memory: self.diagnostics.memory.clone(),
            },
            turn_gate: Arc::clone(&self.turn_gate),
            actor_handle: Arc::clone(&self.actor_handle),
        }
    }
}

impl EneHandle {
    /// Open a ready runtime handle.
    ///
    /// Initializes the LLM provider registry, embedding provider, memory store
    /// (when enabled), tool registry, mind session with `card`, and character
    /// memory warmup **before** returning `Ok`. Config file I/O stays in the
    /// host / `ene-config` — pass an already-loaded config and card.
    pub async fn open(config: EneConfig, card: CharacterCardV3) -> Result<Self, EneRuntimeError> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let cmd_tx = Arc::new(cmd_tx);
        let (event_tx, _event_rx) = broadcast::channel(1024);
        let (diag_tx, _diag_rx) = broadcast::channel(256);

        LlmProviderRegistry::register(Arc::new(ene_ai::OpenAiProviderFactory));

        let mind = config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        ene_mind::CognitionEngine::validate_config(&mind).map_err(EneRuntimeError::from)?;

        let mem_config = config.get_section::<ene_store::StoreConfig>()?;
        let tool_config = config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();
        let needs_embedder = mem_config.enabled || (tool_config.enabled && tool_config.rag.enabled);

        // Fail-closed: memory / tool-RAG features require a working embedder.
        let embedder = if needs_embedder {
            Some(init_embedding(&config)?)
        } else {
            None
        };

        let mut session = ConversationSession::new();
        session.set_card(card);
        if let Some(ref emb) = embedder {
            session.memory.embedding_provider = Some(emb.clone());
        }

        let memory_store = if mem_config.enabled {
            let emb = embedder
                .as_ref()
                .ok_or(EneRuntimeError::MindPrerequisite("embedding provider"))?;
            let store = init_memory_store(&config, emb.as_ref())
                .await
                .map_err(|e| {
                    EneRuntimeError::Memory(ene_store::MemoryError::MemoryStoreConnectionError(e))
                })?;
            store.set_legacy_write_mode(ene_store::LegacyWriteMode::ReadOnly);
            session.memory.memory_store = Some(store.clone());
            Some(store)
        } else {
            None
        };

        let registry = build_tool_registry(&config, memory_store.clone()).await?;
        let tool_rag = embedder
            .as_ref()
            .and_then(|emb| init_tool_rag(&config, emb, &session));
        if let Some(rag) = &tool_rag
            && rag.opts().background_index_on_startup
        {
            let specs = registry.list_tools();
            rag.start_background_indexer(specs);
        }

        // Warmup character memories before returning Ok.
        let warmup_hash = warmup_character_memories_ready(&config, &session).await;

        if let Some(hash) = warmup_hash {
            session.memory.ccv3_memory_hash = Some(hash);
        }

        let memory = MemoryQueryHandle::new(
            session.memory.memory_store.clone(),
            session.memory.embedding_provider.clone(),
        );

        let diagnostics = crate::diagnostics::EneDiagnostics {
            cmd_tx: Arc::clone(&cmd_tx),
            diag_tx: diag_tx.clone(),
            memory: memory.clone(),
        };

        let turn_gate = Arc::new(TurnGate::new());

        let actor = EneActor {
            cmd_rx,
            event_tx: event_tx.clone(),
            diag_tx: diag_tx.clone(),
            turn_gate: Arc::clone(&turn_gate),
            config,
            session,
            registry,
            tool_rag,
            cancel_token: CancellationToken::new(),
            stream_handle: None,
            stream_session_rx: None,
            active_turn: None,
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_compression: None,
            call_tool_tasks: tokio::task::JoinSet::new(),
            terminal_emitted: Arc::new(AtomicBool::new(false)),
        };
        let join = tokio::spawn(actor.run());

        Ok(Self {
            cmd_tx,
            event_tx,
            diag_tx,
            diagnostics,
            turn_gate,
            actor_handle: Arc::new(tokio::sync::Mutex::new(Some(join))),
        })
    }

    /// Subscribe to the chat event stream.
    #[must_use]
    pub fn subscribe(&self) -> EneEventReceiver {
        EneEventReceiver(self.event_tx.subscribe())
    }

    /// Concrete diagnostics facade (pipeline detail, memory, tools).
    #[must_use]
    pub fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics {
        &self.diagnostics
    }

    /// Start a turn. Returns [`RunError::Busy`] if a turn is already in flight
    /// — never silently aborts the active turn.
    pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError> {
        let turn = TurnId::new();
        if !self.turn_gate.try_begin(&turn) {
            return Err(RunError::Busy);
        }
        if self
            .cmd_tx
            .send(EneCommand::Run {
                input: input.into(),
                turn: turn.clone(),
            })
            .is_err()
        {
            self.turn_gate.end();
            return Err(RunError::ActorDead);
        }
        Ok(turn)
    }

    /// Cancel only the given turn. Wrong ids return [`CancelError::TurnMismatch`].
    pub fn cancel(&self, turn: &TurnId) -> Result<(), CancelError> {
        if !self.turn_gate.matches(turn) {
            return Err(CancelError::TurnMismatch);
        }
        self.cmd_tx
            .send(EneCommand::Cancel { turn: turn.clone() })
            .map_err(|_| CancelError::ActorDead)
    }

    /// Send a `Shutdown` command and await the actor's drain.
    pub async fn shutdown(&self, timeout: std::time::Duration) -> Result<(), ShutdownTimeout> {
        if self.cmd_tx.send(EneCommand::Shutdown).is_err() {
            let mut guard = self.actor_handle.lock().await;
            if let Some(join) = guard.take() {
                let _ = join.await;
            }
            return Ok(());
        }

        let mut guard = self.actor_handle.lock().await;
        let Some(join) = guard.as_mut() else {
            return Ok(());
        };
        match tokio::time::timeout(timeout, &mut *join).await {
            Ok(Ok(())) => {
                guard.take();
                Ok(())
            }
            Ok(Err(join_err)) => {
                tracing::warn!(component = "EneHandle", error = %join_err, "Actor task ended with error");
                guard.take();
                Ok(())
            }
            Err(_elapsed) => Err(ShutdownTimeout(timeout)),
        }
    }

    /// Send a permission decision for a pending destructive operation.
    pub fn decide_permission(
        &self,
        request_id: impl Into<RequestId>,
        decision: PermissionDecision,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::PermissionDecision {
                request_id: request_id.into(),
                decision,
            })
            .map_err(|_| ActorDeadError)
    }

    /// Send a user-input response for a pending interactive tool.
    pub fn submit_user_input(
        &self,
        request_id: impl Into<RequestId>,
        response: UserInputResponse,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::UserInputResponse {
                request_id: request_id.into(),
                response,
            })
            .map_err(|_| ActorDeadError)
    }
}

impl Drop for EneHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.cmd_tx) == 1 {
            let _ = self.cmd_tx.send(EneCommand::Shutdown);
        }
    }
}

// ── Actor (internal) ──

struct EneActor {
    cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
    event_tx: broadcast::Sender<EneEvent>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
    turn_gate: Arc<TurnGate>,
    config: EneConfig,
    session: ConversationSession,
    registry: Arc<dyn ToolRegistry>,
    tool_rag: Option<Arc<ene_tool_host::ToolRag>>,
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<Result<ConversationSession, EneRuntimeError>>>,
    active_turn: Option<TurnId>,
    pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    pending_compression: Option<PendingCompressionTask>,
    call_tool_tasks: tokio::task::JoinSet<()>,
    /// Shared with the running stream task; first party to flip emits Terminal.
    terminal_emitted: Arc<AtomicBool>,
}

impl EneActor {
    async fn run(mut self) {
        loop {
            // Reap completed CallTool tasks so the JoinSet
            // does not grow without bound. Bounded by the
            // call rate from `EneCommand::CallTool`, which
            // is interactive.
            while let Some(_joined) = self.call_tool_tasks.try_join_next() {
                // The reply oneshot is sent from inside the
                // task itself; we just drop the JoinError
                // here (it's already been logged by Tokio).
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
                                if !self.handle_command(cmd).await {
                                    break;
                                }
                            }
                            None => break, // All senders dropped
                        }
                    }
                    res = &mut *rx => {
                        // The stream task has finished (or the
                        // sender was dropped). Mirror the
                        // completion bookkeeping that used to
                        // live in `check_stream_completion`.
                        match res {
                            Ok(Ok(updated_session)) => {
                                self.session = updated_session;
                            }
                            Ok(Err(_error)) => {
                                // `run_stream` already emitted the typed
                                // prerequisite failure as the run's terminal
                                // event. Keep the previous session intact.
                            }
                            Err(_) => {
                                // Sender dropped without sending;
                                // keep the previous session.
                            }
                        }
                        self.stream_handle = None;
                        self.stream_session_rx = None;
                        self.active_turn = None;
                        self.turn_gate.end();
                        let _ = self.event_tx.send(EneEvent::StatusChanged {
                            status: EneStatus::Idle,
                        });
                    }
                }
            } else {
                match self.cmd_rx.recv().await {
                    Some(cmd) => {
                        if !self.handle_command(cmd).await {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }

        // Clean up any running stream
        if let Some(handle) = self.stream_handle.take() {
            handle.abort();
        }
        // Abort any in-flight direct CallTool tasks and clear
        // any pending prompt oneshot senders.
        self.call_tool_tasks.abort_all();
        self.drain_pending().await;
    }

    /// Drops every pending permission and user-input
    /// oneshot::Sender, releasing the receiver futures that
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

    async fn handle_command(&mut self, cmd: EneCommand) -> bool {
        match cmd {
            EneCommand::Run { input, turn } => {
                // Single-flight: Busy is enforced on the handle via TurnGate.
                // Never abort an in-flight turn here.
                if self.stream_handle.is_some() {
                    tracing::warn!(
                        component = "EneActor",
                        "Run received while stream active; turn gate should have returned Busy"
                    );
                    return true;
                }
                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                self.terminal_emitted = Arc::new(AtomicBool::new(false));
                self.active_turn = Some(turn.clone());
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Running,
                });
                self.start_stream(input, turn).await;
                true
            }
            EneCommand::Cancel { turn } => {
                if self.active_turn.as_ref() != Some(&turn) {
                    // Mismatch already reported by handle; ignore.
                    return true;
                }
                self.cancel_token.cancel();

                // Abort immediately so the actor loop is not blocked on join.
                // Cancel discards in-flight session updates by design.
                if let Some(handle) = self.stream_handle.take() {
                    handle.abort();
                }
                let _ = self.stream_session_rx.take();

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
                    let _ = self.event_tx.send(EneEvent::Terminal {
                        turn: cancelled_turn,
                        reason: TerminalReason::Cancelled,
                    });
                }
                self.active_turn = None;
                self.turn_gate.end();
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
                true
            }
            EneCommand::Shutdown => {
                self.call_tool_tasks.abort_all();
                self.drain_pending().await;
                false
            }
            EneCommand::SetCharacter { card, reply } => {
                self.session.set_card(*card);
                let _ = reply.send(Ok(()));
                true
            }
            EneCommand::PermissionDecision {
                request_id,
                decision,
            } => {
                let mut guard = self.pending_permissions.lock().await;
                if let Some(tx) = guard.remove(&request_id) {
                    let _ = tx.send(decision);
                }
                true
            }
            EneCommand::UserInputResponse {
                request_id,
                response,
            } => {
                let mut guard = self.pending_user_inputs.lock().await;
                if let Some(tx) = guard.remove(&request_id) {
                    let _ = tx.send(response);
                }
                true
            }
            EneCommand::ManualSplit { reply } => {
                let result = self.handle_manual_split().await;
                let _ = reply.send(result);
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
                    ),
                    current_turn_count: self.session.current_turn_count() as u32,
                    session_started_at: self.session.session_started_at(),
                };
                let _ = reply.send(snapshot);
                true
            }
            EneCommand::ListTools { reply } => {
                let tools = self.registry.list_tools();
                let _ = reply.send(tools);
                true
            }
            EneCommand::CallTool {
                name,
                arguments,
                reply,
            } => {
                let registry = self.registry.clone();
                // Track the spawned task in `call_tool_tasks`
                // so it can be aborted on `Shutdown` and
                // reaped on completion. The reply oneshot
                // send is no longer silent on send-failure:
                // the task is dropped alongside the JoinSet
                // entry, so the caller is implicitly notified
                // (the receiver sees `Closed`).
                self.call_tool_tasks.spawn(async move {
                    let result = registry
                        .call_tool(&name, &arguments)
                        .await
                        .map_err(EneRuntimeError::from);
                    let _ = reply.send(result);
                });
                true
            }
            EneCommand::InvalidateToolIndex => {
                self.tool_rag = None;
                true
            }
            EneCommand::SetCcv3MemoryHash { hash, reply } => {
                self.session.memory.ccv3_memory_hash = Some(hash);
                let _ = reply.send(());
                true
            }
        }
    }

    async fn start_stream(&mut self, user_input: String, turn: TurnId) {
        // Create the provider before mutating history so a failed open leaves
        // the session unchanged.
        let provider = match self.create_provider() {
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
                    let _ = self.event_tx.send(EneEvent::Terminal {
                        turn: turn.clone(),
                        reason: TerminalReason::Failed {
                            message: e.to_string(),
                        },
                    });
                }
                self.active_turn = None;
                self.turn_gate.end();
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
                return;
            }
        };

        self.apply_pending_compression();

        self.session.record_user_input();
        self.session.add_user_message(&user_input);

        self.check_and_perform_split(&user_input);

        let config = self.config.clone();
        let session = self.session.clone();
        let embedder = self.session.memory.embedding_provider.clone();
        let registry = self.registry.clone();
        let tool_rag = self.tool_rag.clone();
        let event_tx = self.event_tx.clone();
        let diag_tx = self.diag_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let pending_permissions = self.pending_permissions.clone();
        let pending_user_inputs = self.pending_user_inputs.clone();
        let terminal_emitted = self.terminal_emitted.clone();
        let turn_for_stream = turn;

        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let handle = tokio::spawn(async move {
            let result = streaming::run_stream(streaming::StreamContext {
                config,
                session,
                user_input,
                embedder,
                registry,
                tool_rag,
                provider,
                event_tx,
                diag_tx,
                cancel_token,
                pending_permissions,
                pending_user_inputs,
                terminal_emitted,
                turn: turn_for_stream,
            })
            .await;
            let _ = session_tx.send(result);
        });
        self.stream_handle = Some(handle);
    }

    fn create_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        let provider_config = self.config.get_section::<ene_ai::ProviderConfig>()?;
        LlmProviderRegistry::create_provider(&provider_config.name, &self.config)
            .map(Arc::from)
            .map_err(EneRuntimeError::from)
    }

    // ── Split management ──

    async fn handle_manual_split(&mut self) -> Result<SplitResult, EneRuntimeError> {
        let mind = self
            .config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        if !mind.context.compression_enabled {
            return Err(EneRuntimeError::CompressionRequired);
        }
        if self.session.history().is_empty() {
            return Err(EneRuntimeError::from(EneSessionError::SplitNotNeeded));
        }
        self.handle_manual_compression().await
    }

    async fn handle_manual_compression(&mut self) -> Result<SplitResult, EneRuntimeError> {
        let Some(store) = self.session.memory.memory_store.clone() else {
            return Err(EneRuntimeError::from(EneSessionError::SplitNotNeeded));
        };
        let provider = self.create_provider()?;
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
        let result = execute_compression(store, provider, input).await?;
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

    fn check_and_perform_split(&mut self, _user_input: &str) {
        // Product path is compression-only; hard session-ID minting is not used.
        // compression_enabled is required by MindConfig validation at open.
        self.check_and_trigger_compression();
    }

    fn check_and_trigger_compression(&mut self) {
        let mem_config = match self.config.get_section::<ene_store::StoreConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        let mind = match self.config.get_section::<ene_mind::MindConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        if !mem_config.enabled || !mind.context.compression_enabled {
            return;
        }
        if self.pending_compression.is_some() {
            return;
        }

        let turn_count = self.session.current_turn_count();
        let history_len = self.session.history().len();
        if evaluate_compression_trigger(&mind.context, turn_count, history_len).is_none() {
            return;
        }

        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let provider = match self.create_provider() {
            Ok(p) => p,
            Err(_) => return,
        };

        let recent_cap = mind.context.recent_turns.saturating_mul(2).max(2);
        let history = self.session.history();
        if history.len() <= recent_cap {
            return;
        }
        let compress_count = history.len().saturating_sub(recent_cap);
        if compress_count < MIN_MESSAGES_TO_COMPRESS {
            return;
        }
        let turns: Vec<MindHistoryEntry> = history[..compress_count].to_vec();
        let turn_end = turn_count as i32;
        let turn_start = (turn_end - (compress_count as i32 / 2).max(1)).max(0);

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
        spawn_compression_task(&mut self.pending_compression, store, provider, input);
    }

    fn apply_pending_compression(&mut self) {
        if let Some(result) = poll_compression_result(&mut self.pending_compression) {
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
                    self.spawn_chapter_rollup_if_needed();
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
            .map(|c| c.context.recent_turns.saturating_mul(2).max(2))
            .unwrap_or(16);
        let history_len = self.session.history().len();
        if history_len > recent_cap {
            self.session.trim_history_keep_last(recent_cap);
        }
    }

    fn spawn_chapter_rollup_if_needed(&self) {
        let mind = match self.config.get_section::<ene_mind::MindConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let Ok(provider) = self.create_provider() else {
            return;
        };
        let session_id = self.session.memory.session_id.to_string();
        let character_name = self.session.card_name().to_string();
        let user_name = self.config.user_name.clone();
        let context = mind.context.clone();
        tokio::spawn(async move {
            if let Err(error) = maybe_roll_up_chapter(
                store.as_ref(),
                provider,
                &session_id,
                &character_name,
                &user_name,
                &context,
            )
            .await
            {
                tracing::warn!(
                    component = "ContextCompression",
                    error = %error,
                    "Chapter rollup failed"
                );
            }
        });
    }

    fn mind_history_entries(&self) -> Vec<MindHistoryEntry> {
        self.session.history().to_vec()
    }
}

// ── Factory / init helpers (moved from runtime.rs) ──

async fn warmup_character_memories_ready(
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
        store,
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

/// Builds the active composite tool registry based on workspace config.
/// Spawns per-tool DB IPC servers before starting tool processes.
async fn build_tool_registry(
    config: &EneConfig,
    memory_store: Option<Arc<ene_store::MemoryStore>>,
) -> Result<Arc<dyn ToolRegistry>, EneRuntimeError> {
    let mut db_tokens = std::collections::HashMap::new();
    if let Some(store) = &memory_store {
        #[cfg(any(unix, windows))]
        {
            let tool_config = config
                .get_section::<ene_tool_host::ToolConfig>()
                .unwrap_or_default();

            let db = store.connection().clone();

            let socket_dir = ene_config::paths::tool_socket_dir();
            std::fs::create_dir_all(&socket_dir).map_err(|e| {
                EneRuntimeError::Tool(ene_tool_host::ToolHostError::ExecutionFailed {
                    message: format!("Failed to create socket dir: {e}"),
                })
            })?;

            for (name, entry) in &tool_config.list {
                if !entry.enable {
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
                // DB IPC connection. The token is handed to the tool
                // binary via SandboxConfigData::db_auth_token so only
                // the legitimate child process can authenticate.
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
                let _ = reader.read_exact(&mut token_out);
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
    }

    ToolHostManager::start_full(config, db_tokens)
        .await
        .map_err(EneRuntimeError::Tool)
}

fn init_embedding(
    config: &EneConfig,
) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
    let provider_config = config
        .get_section::<ene_ai::ProviderConfig>()
        .map_err(|e| {
            ene_ai::EmbeddingError::Init(format!("Failed to load provider config: {e}"))
        })?;

    if provider_config.embedding.backend.as_str() == "local" {
        let local_cfg = &provider_config.embedding.local;
        let model_dir = ene_config::models_dir();
        let provider =
            ene_ai::create_local_provider(&local_cfg.model, &local_cfg.quantization, model_dir)?;
        Ok(Arc::from(provider))
    } else {
        let base_url = provider_config.resolve_base_url().map_err(|e| {
            ene_ai::EmbeddingError::Init(format!(
                "Failed to resolve base URL for cloud embedding: {e}"
            ))
        })?;
        let api_key = provider_config.resolve_api_key();
        let query_prefix = provider_config.embedding.query_prefix.clone();
        Ok(Arc::new(ene_ai::CloudEmbeddingProvider::new(
            &base_url,
            &api_key,
            &provider_config.embedding.cloud.model,
            provider_config.embedding.cloud.dimensions,
            query_prefix,
        )))
    }
}

async fn init_memory_store(
    config: &EneConfig,
    embedder: &dyn ene_ai::EmbeddingProvider,
) -> Result<Arc<ene_store::MemoryStore>, String> {
    let db_path = config
        .get_section::<ene_store::StoreConfig>()
        .unwrap_or_default()
        .resolve_memory_db_path(&config.character);

    if let Some(parent) = db_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create memory DB directory: {e}"))?;
    }

    let dims = embedder.dimensions();
    let store = ene_store::MemoryStore::open(&db_path, dims)
        .await
        .map_err(|e| format!("Failed to open memory store: {e}"))?;

    Ok(Arc::new(store))
}

/// Builds the `ToolRag` pipeline from the current config, embedder, and session state.
///
/// Returns `None` when the pipeline is disabled. Logs an error
/// and returns `None` when the config has an invalid `forced`
/// tool name (the malformed entry is dropped so a single bad
/// name does not prevent the rest of the tool RAG from
/// working — but the error is surfaced via `tracing` so the
/// operator can see it in the logs).
fn init_tool_rag(
    config: &EneConfig,
    embedder: &Arc<dyn ene_ai::EmbeddingProvider>,
    session: &ConversationSession,
) -> Option<Arc<ene_tool_host::ToolRag>> {
    let rag_config = config
        .get_section::<ene_tool_host::ToolConfig>()
        .map(|tc| tc.rag)
        .unwrap_or_default();

    if !rag_config.enabled {
        return None;
    }

    let store = session.memory.memory_store.clone();
    let opts = match ene_tool_host::ToolRagOptions::try_from(rag_config) {
        Ok(o) => o,
        Err(e) => {
            tracing::error!(
                "[ToolRag] Invalid tool name in rag.forced config: {e}; building pipeline without forced tools"
            );
            // Build with the default options (which has a
            // sane `forced` set compiled in). The bad name
            // is logged but does not block the pipeline.
            ene_tool_host::ToolRagOptions::default()
        }
    };
    Some(Arc::new(ene_tool_host::ToolRag::new(
        embedder.clone(),
        store,
        opts,
    )))
}

#[cfg(test)]
mod tests {
    // Contract tests for open/Busy/TurnId live in tests/api_v2_contract.rs.
}
