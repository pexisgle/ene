#[cfg(any(unix, windows))]
use crate::db_server::DbIpcServer;
use crate::diagnostics::{DiagnosticEvent, MemoryQueryHandle};
use crate::error::EneRuntimeError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::{CancelError, RequestId, RunError, TurnId};
use chrono::{DateTime, Utc};
use ene_ai::{AiTaskKind, LlmProviderRegistry, create_task_chat_provider};
use ene_config::CharacterCardV3;
use ene_config::EneConfig;
use ene_mind::commitments::CommitmentLedger;
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
use ene_tool_rag::{ToolRag, ToolRagConfig, ToolRagOptions};
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
        /// Active turn for call-context propagation. `None` for
        /// diagnostic / background tool calls outside a turn.
        turn: Option<TurnId>,
        /// Reply channel.
        reply: oneshot::Sender<Result<String, EneRuntimeError>>,
    },
    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    InvalidateToolIndex,
    /// Persist the `CCv3` character-memory content hash after startup warmup.
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
    /// Update host-side proactive observation snapshot (#166).
    UpdateProactiveObservation {
        /// Normalized observation from desktop (no raw screenshots).
        observation: ene_mind::ProactiveObservation,
    },
    /// Hot-update proactive policy (#103). Provider routing comes from [`AiConfig`].
    UpdateProactiveSettings {
        /// Mind proactive policy.
        mind: ene_mind::ProactiveConfig,
    },
    /// Hot-update Features-tab settings (mind / store / tools / RAG) without
    /// tearing down the local proactive GGUF.
    UpdateFeatureSettings {
        /// Boxed payload to keep [`EneCommand`] small.
        settings: Box<FeatureSettingsUpdate>,
    },
    /// Summarize a screen RGB capture with the local vision (mmproj) model.
    SummarizeScreenImage {
        /// Image width in pixels.
        width: u32,
        /// Image height in pixels.
        height: u32,
        /// Tight RGB8 buffer (`width * height * 3`).
        rgb: Vec<u8>,
        /// Reply channel.
        reply: oneshot::Sender<Result<String, String>>,
    },
}

/// Payload for [`EneCommand::UpdateFeatureSettings`].
#[derive(Debug, Clone)]
pub struct FeatureSettingsUpdate {
    /// Full mind section (emotion + proactive).
    pub mind: ene_mind::MindConfig,
    /// Long-term memory store section.
    pub store: ene_store::StoreConfig,
    /// Tool host section.
    pub tools: ene_tool_host::ToolConfig,
    /// Tool RAG section.
    pub rag: ToolRagConfig,
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
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The raw text delta.
        delta: String,
    },
    /// Presentation cues (expression / emote) for the active turn.
    Performance {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Cue list (usually one expression).
        cues: Vec<ene_mind::PerformanceCue>,
        /// How the cues were chosen.
        source: ene_mind::CueSource,
    },
    /// A tool call has been requested by the LLM.
    ToolCallStart {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The tool name (e.g. "fs.write").
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// A tool call has completed with its result.
    ToolCallResult {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// The tool name.
        name: String,
        /// The tool's output as a string.
        result: String,
    },
    /// A destructive operation requires user approval before execution.
    PermissionRequired {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
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
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Unique identifier for this input request.
        request_id: RequestId,
        /// The prompt describing the question, options, and free-text allowance.
        prompt: ene_tool_proto::UserInputPrompt,
    },
    /// Thin signal that rolling context compression completed for this turn.
    ContextCompressed {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Compression level label (e.g. "scene").
        level: String,
    },
    /// Terminal event for a run: emitted exactly once after `after_turn` completes.
    Terminal {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
        /// Why the run terminated.
        reason: TerminalReason,
    },
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
    /// A turn has started streaming (after provider open succeeds).
    TurnStarted {
        /// Active turn.
        turn: TurnId,
        /// Who initiated this turn.
        origin: crate::types::TurnOrigin,
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
    const fn new() -> Self {
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
        let (diag_tx, diag_rx) = broadcast::channel(256);

        LlmProviderRegistry::register(Arc::new(ene_ai::OpenAiProviderFactory));

        let mind = config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default();
        ene_mind::CognitionEngine::validate_config(&mind).map_err(EneRuntimeError::from)?;

        let mem_config = config.get_section::<ene_store::StoreConfig>()?;
        let tool_config = config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();
        let rag_config = config.get_section::<ToolRagConfig>().unwrap_or_default();
        let needs_embedder = mem_config.enabled || (tool_config.enabled && rag_config.enabled);

        // Prefetch configured GGUF weights in parallel before backends load them.
        if let Ok(ai_config) = config.get_section::<ene_ai::AiConfig>() {
            let needs_decision = mind.proactive.enabled;
            if (needs_embedder || needs_decision)
                && let Err(e) =
                    ene_ai::prefetch_configured_gguf(&ai_config, needs_embedder, needs_decision)
                        .await
            {
                tracing::warn!(
                    component = "GgufPrefetch",
                    error = %e,
                    "GGUF prefetch failed; will retry on load"
                );
            }
        }

        // Fail-closed: memory / tool-RAG features require a working embedder.
        let embedder = if needs_embedder {
            Some(init_embedding(&config)?)
        } else {
            None
        };

        let mut session = ConversationSession::new();
        session.set_card(&card);
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
            session.memory.memory_store = Some(store.clone());
            Some(store)
        } else {
            None
        };

        let registry = build_tool_registry(&config, memory_store.clone()).await?;
        let tool_rag = match embedder.as_ref() {
            Some(emb) => init_tool_rag(&config, emb, &session)?,
            None => None,
        };
        if let Some(rag) = &tool_rag {
            let specs = registry.list_tools();
            let profiles = registry.list_rag_profiles();
            if rag.opts().background_index_on_startup {
                tracing::info!(
                    component = "Bootstrap",
                    "Warming up Tool RAG index in background..."
                );
                rag.start_background_indexer(specs, profiles);
            } else {
                tracing::debug!(
                    component = "Bootstrap",
                    "Skipping Tool RAG startup warmup (background_index_on_startup=false)"
                );
            }
        }

        // Warmup character memories before returning Ok.
        let warmup_hash = warmup_character_memories_ready(&config, &session).await;

        if let Some(hash) = warmup_hash {
            session.memory.ccv3_memory_hash = Some(hash);
        }

        let mind_memory = config
            .get_section::<ene_mind::MindConfig>()
            .unwrap_or_default()
            .memory;
        let memory = MemoryQueryHandle::new(
            session.memory.memory_store.clone(),
            session.memory.embedding_provider.clone(),
            mind_memory,
        );

        let diagnostics = crate::diagnostics::EneDiagnostics {
            cmd_tx: Arc::clone(&cmd_tx),
            diag_tx: diag_tx.clone(),
            memory,
        };

        let turn_gate = Arc::new(TurnGate::new());
        let (classifier_tx, classifier_rx) = mpsc::unbounded_channel();
        let (memory_writer_tx, memory_writer_rx) = mpsc::unbounded_channel();

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
            classifier_tasks: tokio::task::JoinSet::new(),
            memory_writer_tasks: tokio::task::JoinSet::new(),
            vision_tasks: tokio::task::JoinSet::new(),
            classifier_rx,
            memory_writer_rx,
            terminal_emitted: Arc::new(AtomicBool::new(false)),
            classifier_tx,
            memory_writer_tx,
            _diag_rx: diag_rx,
            proactive: crate::proactive::ProactiveScheduler::default(),
            proactive_decision_rx: None,
            proactive_decision_handle: None,
            proactive_llm: None,
            active_origin: crate::types::TurnOrigin::User,
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
    pub fn subscribe(&self) -> EneEventReceiver {
        EneEventReceiver(self.event_tx.subscribe())
    }

    /// Concrete diagnostics facade (pipeline detail, memory, tools).
    pub const fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics {
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
            drop(guard);
            return Ok(());
        }

        let mut guard = self.actor_handle.lock().await;
        let Some(join) = guard.as_mut() else {
            drop(guard);
            return Ok(());
        };
        match tokio::time::timeout(timeout, &mut *join).await {
            Ok(Ok(())) => {
                guard.take();
                drop(guard);
                Ok(())
            }
            Ok(Err(join_err)) => {
                tracing::warn!(component = "EneHandle", error = %join_err, "Actor task ended with error");
                guard.take();
                drop(guard);
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

    /// Push a privacy-safe proactive observation from the host (#166 / #168).
    pub fn update_proactive_observation(
        &self,
        observation: ene_mind::ProactiveObservation,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::UpdateProactiveObservation { observation })
            .map_err(|_| ActorDeadError)
    }

    /// Hot-update proactive policy in the running actor.
    ///
    /// Prefer [`Self::update_feature_settings`] when emotion / store / tools
    /// also change — this path only patches `mind.proactive` and does not
    /// reload the local decision model.
    pub fn update_proactive_settings(
        &self,
        mind: ene_mind::ProactiveConfig,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::UpdateProactiveSettings { mind })
            .map_err(|_| ActorDeadError)
    }

    /// Hot-update Features-tab sections (mind / store / tools / RAG).
    ///
    /// Does not tear down local GGUF handles. Tool process registry is rebuilt
    /// when the tools section changes.
    pub fn update_feature_settings(
        &self,
        mind: ene_mind::MindConfig,
        store: ene_store::StoreConfig,
        tools: ene_tool_host::ToolConfig,
        rag: ToolRagConfig,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::UpdateFeatureSettings {
                settings: Box::new(FeatureSettingsUpdate {
                    mind,
                    store,
                    tools,
                    rag,
                }),
            })
            .map_err(|_| ActorDeadError)
    }

    /// Summarize a screen RGB capture via the local vision model (Gemma + mmproj).
    pub async fn summarize_screen_image(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
    ) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SummarizeScreenImage {
                width,
                height,
                rgb,
                reply: tx,
            })
            .map_err(|_| "actor dead".to_string())?;
        rx.await.map_err(|_| "actor dropped reply".to_string())?
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
    tool_rag: Option<Arc<ToolRag>>,
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<streaming::StreamOutcome>>,
    active_turn: Option<TurnId>,
    pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    pending_compression: Option<PendingCompressionTask>,
    call_tool_tasks: tokio::task::JoinSet<()>,
    classifier_tasks: tokio::task::JoinSet<()>,
    memory_writer_tasks: tokio::task::JoinSet<()>,
    /// In-flight screen-summary vision jobs (must not block the command loop).
    vision_tasks: tokio::task::JoinSet<()>,
    classifier_rx: mpsc::UnboundedReceiver<tokio::task::JoinHandle<()>>,
    memory_writer_rx: mpsc::UnboundedReceiver<tokio::task::JoinHandle<()>>,
    /// Sender for classifier `JoinHandles` from the stream task.
    classifier_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    /// Sender for deferred memory-writer `JoinHandles` from the stream task.
    memory_writer_tx: mpsc::UnboundedSender<tokio::task::JoinHandle<()>>,
    /// Shared with the running stream task; first party to flip emits Terminal.
    terminal_emitted: Arc<AtomicBool>,
    /// Held so the broadcast channel retains buffered diagnostic events until the
    /// first subscriber attaches via [`EneHandle::diagnostics().subscribe()`].
    _diag_rx: broadcast::Receiver<DiagnosticEvent>,
    /// Proactive speech scheduler state (#166).
    proactive: crate::proactive::ProactiveScheduler,
    /// In-flight decision result channel.
    proactive_decision_rx: Option<oneshot::Receiver<crate::proactive::ProactiveDecisionResult>>,
    /// Join handle for the in-flight decision task (aborted on user turn / shutdown).
    proactive_decision_handle: Option<tokio::task::JoinHandle<()>>,
    /// Local / cloud decision provider handles (lazy).
    proactive_llm: Option<ene_ai::ProactiveLlmHandles>,
    /// Origin of the active stream turn (for cancel Terminal).
    active_origin: crate::types::TurnOrigin,
}

impl EneActor {
    async fn run(mut self) {
        loop {
            // Reap completed CallTool tasks so the JoinSet
            // does not grow without bound. Bounded by the
            // call rate from `EneCommand::CallTool`, which
            // is interactive.
            while let Some(joined) = self.call_tool_tasks.try_join_next() {
                if let Err(e) = joined {
                    tracing::error!(
                        component = "CallToolReaper",
                        error = %e,
                        "CallTool task panicked"
                    );
                }
            }

            // Reap completed classifier tasks.
            while let Some(joined) = self.classifier_tasks.try_join_next() {
                if let Err(e) = joined {
                    tracing::error!(
                        component = "ClassifierReaper",
                        error = %e,
                        "Classifier task panicked"
                    );
                }
            }

            // Reap completed deferred memory-writer tasks.
            while let Some(joined) = self.memory_writer_tasks.try_join_next() {
                if let Err(e) = joined {
                    tracing::error!(
                        component = "MemoryWriterReaper",
                        error = %e,
                        "Deferred memory-writer task panicked"
                    );
                }
            }

            while let Some(joined) = self.vision_tasks.try_join_next() {
                if let Err(e) = joined {
                    tracing::error!(
                        component = "VisionReaper",
                        error = %e,
                        "Screen summary vision task panicked"
                    );
                }
            }

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
                self.memory_writer_tasks.spawn(async move {
                    if let Err(e) = handle.await {
                        tracing::error!(
                            component = "MemoryWriter",
                            error = %e,
                            "Deferred memory-writer task panicked"
                        );
                    }
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
                                if !self.handle_command(cmd).await {
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
                        }
                        self.stream_handle = None;
                        self.stream_session_rx = None;
                        self.active_turn = None;
                        self.active_origin = crate::types::TurnOrigin::User;
                        self.turn_gate.end();
                        let _ = self.event_tx.send(EneEvent::StatusChanged {
                            status: EneStatus::Idle,
                        });
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
                                    if !self.handle_command(cmd).await {
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
                                    if !self.handle_command(cmd).await {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                        () = tokio::time::sleep(tick) => {
                            self.maybe_spawn_proactive_decision().await;
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
        self.vision_tasks.abort_all();
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

    async fn ensure_proactive_llm(&mut self) -> Result<(), String> {
        if self.proactive_llm.is_some() {
            return Ok(());
        }
        let ai_cfg = self
            .config
            .get_section::<ene_ai::AiConfig>()
            .map_err(|e| format!("AI config unavailable: {e}"))?;
        match ene_ai::build_proactive_llm_handles(&ai_cfg).await {
            Ok(handles) => {
                tracing::info!(
                    component = "Proactive",
                    decision_backend = ?handles.decision_kind,
                    vision = handles.local().is_some_and(|l| l.supports_vision()),
                    "Proactive decision provider ready"
                );
                self.proactive_llm = Some(handles);
                Ok(())
            }
            Err(e) => Err(format!("Failed to start proactive decision provider: {e}")),
        }
    }

    async fn summarize_screen_rgb(
        &mut self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        reply: oneshot::Sender<Result<String, String>>,
    ) {
        const MAX_PIXELS: u64 = 1920 * 1080;
        let width_u = u64::from(width);
        let height_u = u64::from(height);
        let expected = width_u.saturating_mul(height_u).saturating_mul(3);
        if width == 0 || height == 0 {
            let _ = reply.send(Err("invalid screen image dimensions".to_string()));
            return;
        }
        if width_u.saturating_mul(height_u) > MAX_PIXELS {
            let _ = reply.send(Err(format!(
                "screen image too large ({width}x{height}; max {MAX_PIXELS} pixels)"
            )));
            return;
        }
        let Ok(expected_len) = usize::try_from(expected) else {
            let _ = reply.send(Err("screen image byte length overflows usize".to_string()));
            return;
        };
        if rgb.len() != expected_len {
            let _ = reply.send(Err(format!(
                "rgb buffer length mismatch (got {}, expected {expected_len})",
                rgb.len()
            )));
            return;
        }
        if self.stream_handle.is_some() || self.proactive_decision_rx.is_some() {
            let _ = reply.send(Err("runtime busy".to_string()));
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
            let _ = reply.send(Err(e));
            return;
        }
        let Some(handles) = self.proactive_llm.as_ref() else {
            let _ = reply.send(Err("proactive LLM handles missing after ensure".to_string()));
            return;
        };
        let Some(local) = handles.local().cloned() else {
            let _ = reply.send(Err(format!(
                "local proactive model is not available (decision_backend={:?})",
                handles.decision_kind
            )));
            return;
        };
        if !local.supports_vision() {
            let _ = reply.send(Err("local model has no vision mmproj loaded".to_string()));
            return;
        }

        let prompts = ene_config::PromptLibrary::load(&prompt_language);
        let system = prompts.proactive().screen_summary_system.trim().to_string();
        let user = prompts.proactive().screen_summary_user.trim().to_string();
        self.vision_tasks.spawn(async move {
            let result = local
                .summarize_rgb(width, height, rgb, &system, &user)
                .await
                .map_err(|e| e.to_string());
            let _ = reply.send(result);
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
            let _ = tx.send(result);
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
        self.drain_pending().await;
        self.cancel_token = CancellationToken::new();
        self.terminal_emitted = Arc::new(AtomicBool::new(false));
        self.active_turn = Some(turn.clone());
        self.active_origin = crate::types::TurnOrigin::Proactive;
        let _ = self.event_tx.send(EneEvent::StatusChanged {
            status: EneStatus::Running,
        });
        let generation_timeout =
            std::time::Duration::from_secs(mind.proactive.generation_timeout_seconds.max(1));
        self.start_stream(
            String::new(),
            turn,
            crate::types::TurnOrigin::Proactive,
            false,
            mind.proactive.allow_tools,
            Some(hint),
            Some(generation_timeout),
        );
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
                // Discard any in-flight proactive decision.
                self.proactive.on_user_turn_started();
                self.abort_proactive_decision();
                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                self.terminal_emitted = Arc::new(AtomicBool::new(false));
                self.active_turn = Some(turn.clone());
                self.active_origin = crate::types::TurnOrigin::User;
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Running,
                });
                self.start_stream(
                    input,
                    turn,
                    crate::types::TurnOrigin::User,
                    true,
                    true,
                    None,
                    None,
                );
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
                                    component = "EneActor",
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
                            let _ = handle.await;
                        }
                    }
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
                        origin: self.active_origin,
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
                self.classifier_tasks.abort_all();
                self.memory_writer_tasks.abort_all();
                self.call_tool_tasks.abort_all();
                self.vision_tasks.abort_all();
                self.abort_proactive_decision();
                if let Some(handles) = self.proactive_llm.take() {
                    handles.shutdown().await;
                }
                self.drain_pending().await;
                false
            }
            EneCommand::SetCharacter { card, reply } => {
                self.session.set_card(&card);
                self.proactive.reset_session();
                self.abort_proactive_decision();
                let _ = reply.send(Ok(()));
                true
            }
            EneCommand::UpdateProactiveObservation { observation } => {
                self.proactive.observation = observation;
                true
            }
            EneCommand::UpdateProactiveSettings { mind } => {
                if let Ok(mut mind_cfg) = self.config.get_section::<ene_mind::MindConfig>() {
                    mind_cfg.proactive = mind;
                    let _ = self.config.set_section(&mind_cfg);
                }
                self.abort_proactive_decision();
                true
            }
            EneCommand::UpdateFeatureSettings { settings } => {
                let FeatureSettingsUpdate {
                    mind,
                    store,
                    tools,
                    rag,
                } = *settings;
                let prev_tools = self
                    .config
                    .get_section::<ene_tool_host::ToolConfig>()
                    .unwrap_or_default();
                let tools_changed = tool_enable_set_changed(&prev_tools, &tools);

                let _ = self.config.set_section(&mind);
                let _ = self.config.set_section(&store);
                let _ = self.config.set_section(&tools);
                let _ = self.config.set_section(&rag);
                self.abort_proactive_decision();

                if tools_changed {
                    match build_tool_registry(
                        &self.config,
                        self.session.memory.memory_store.clone(),
                    )
                    .await
                    {
                        Ok(registry) => {
                            self.registry = registry;
                            tracing::info!(
                                component = "EneActor",
                                "Tool registry rebuilt after Features update"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                component = "EneActor",
                                error = %e,
                                "Failed to rebuild tool registry after Features update"
                            );
                        }
                    }
                }
                true
            }
            EneCommand::SummarizeScreenImage {
                width,
                height,
                rgb,
                reply,
            } => {
                self.summarize_screen_rgb(width, height, rgb, reply).await;
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
                drop(guard);
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
                drop(guard);
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
                        self.config
                            .get_section::<ene_mind::MindConfig>()
                            .unwrap_or_default()
                            .memory,
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
                turn,
                reply,
            } => {
                let registry = self.registry.clone();
                let session_id = self.session.memory.session_id.to_string();
                self.call_tool_tasks.spawn(async move {
                    if let Some(ref turn) = turn {
                        let call_ctx = ene_tool_proto::CallContext {
                            conversation_id: session_id,
                            turn_id: turn.to_string(),
                        };
                        registry.set_call_context(&call_ctx).await;
                    }
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

    fn start_stream(
        &mut self,
        user_input: String,
        turn: TurnId,
        origin: crate::types::TurnOrigin,
        record_user_message: bool,
        allow_tools: bool,
        runtime_directive: Option<String>,
        generation_timeout: Option<std::time::Duration>,
    ) {
        // Create the provider before mutating history so a failed open leaves
        // the session unchanged.
        let provider = match if origin == crate::types::TurnOrigin::Proactive {
            self.create_proactive_provider()
        } else {
            self.create_provider()
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
                    let _ = self.event_tx.send(EneEvent::Terminal {
                        turn,
                        origin,
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

        if record_user_message {
            self.session.record_user_input();
            self.session.add_user_message(&user_input);
            self.check_and_perform_split(&user_input);
        }

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
        let turn_for_stream = turn.clone();
        let classifier_tx = self.classifier_tx.clone();
        let memory_writer_tx = self.memory_writer_tx.clone();
        self.active_origin = origin;

        let _ = self.event_tx.send(EneEvent::TurnStarted {
            turn: turn_for_stream.clone(),
            origin,
        });

        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let handle = tokio::spawn(async move {
            let outcome = streaming::run_stream_cognitive(streaming::StreamContext {
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
                origin,
                allow_tools,
                runtime_directive,
                generation_timeout,
                classifier_tx,
                memory_writer_tx,
            })
            .await;
            let _ = session_tx.send(outcome);
        });
        self.stream_handle = Some(handle);
    }

    fn create_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        create_task_chat_provider(&self.config, AiTaskKind::Chat)
            .map(Arc::from)
            .map_err(EneRuntimeError::from)
    }

    fn create_proactive_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        create_task_chat_provider(&self.config, AiTaskKind::Proactive)
            .map(Arc::from)
            .map_err(EneRuntimeError::from)
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
        self.check_and_trigger_compression();
    }

    fn check_and_trigger_compression(&mut self) {
        let Ok(mem_config) = self.config.get_section::<ene_store::StoreConfig>() else {
            return;
        };
        if !mem_config.enabled {
            return;
        }
        let Ok(mind) = self.config.get_section::<ene_mind::MindConfig>() else {
            return;
        };
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
        let Ok(provider) = self.create_provider() else {
            return;
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
            config: mind.context,
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
            .map_or(16, |c| c.context.recent_turns.saturating_mul(2).max(2));
        let history_len = self.session.history().len();
        if history_len > recent_cap {
            self.session.trim_history_keep_last(recent_cap);
        }
    }

    fn spawn_chapter_rollup_if_needed(&self) {
        let Ok(mind) = self.config.get_section::<ene_mind::MindConfig>() else {
            return;
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
        let context = mind.context;
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

fn tool_enable_set_changed(
    prev: &ene_tool_host::ToolConfig,
    next: &ene_tool_host::ToolConfig,
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
    use ene_ai::ResolvedEmbedding;

    let ai_config = config
        .get_section::<ene_ai::AiConfig>()
        .map_err(|e| ene_ai::EmbeddingError::Init(format!("Failed to load AI config: {e}")))?;
    match ai_config
        .resolve_embedding()
        .map_err(|e| ene_ai::EmbeddingError::Init(e.to_string()))?
    {
        ResolvedEmbedding::Local(local) => {
            let provider = ene_ai::create_local_provider(&local)?;
            Ok(Arc::from(provider))
        }
        ResolvedEmbedding::Cloud {
            base_url,
            api_key,
            model,
            dimensions,
            query_prefix,
        } => Ok(Arc::new(ene_ai::CloudEmbeddingProvider::new(
            &base_url,
            &api_key,
            &model,
            dimensions,
            query_prefix,
        ))),
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
/// Returns `None` when the pipeline is disabled. Invalid `forced` tool
/// names fail `EneHandle::open`.
fn init_tool_rag(
    config: &EneConfig,
    embedder: &Arc<dyn ene_ai::EmbeddingProvider>,
    session: &ConversationSession,
) -> Result<Option<Arc<ToolRag>>, EneRuntimeError> {
    let rag_config = config.get_section::<ToolRagConfig>().unwrap_or_default();

    if !rag_config.enabled {
        return Ok(None);
    }

    let store = session.memory.memory_store.clone();
    let opts = ToolRagOptions::from_config(rag_config)?;
    Ok(Some(Arc::new(ToolRag::new(embedder.clone(), store, opts))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config_memory_off() -> EneConfig {
        let mut config = EneConfig::default();
        let store = ene_store::StoreConfig {
            enabled: false,
            ..Default::default()
        };
        config.set_section(&store).expect("store config merges");
        let tools = ene_tool_host::ToolConfig {
            enabled: false,
            ..Default::default()
        };
        let _ = config.set_section(&tools);
        let ai = ene_ai::AiConfig::default();
        let _ = config.set_section(&ai);
        config
    }

    fn test_card() -> CharacterCardV3 {
        CharacterCardV3::default()
    }

    #[tokio::test]
    async fn broadcast_channels_emit_lag_on_overflow() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");

        // Test event_tx buffer overflow (capacity is 1024)
        let mut event_rx = handle.subscribe();

        // Send 1025 events to exceed the buffer capacity of 1024
        for i in 0..1025 {
            let _ = handle.event_tx.send(EneEvent::TextDelta {
                turn: TurnId::new(),
                origin: crate::types::TurnOrigin::User,
                delta: format!("delta {i}"),
            });
        }

        // Try to receive and it should return RecvError::Lagged
        let res = event_rx.recv().await;
        assert!(
            matches!(
                res,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
            ),
            "Expected RecvError::Lagged for event channel overflow, got {res:?}"
        );

        // Test diag_tx buffer overflow (capacity is 256)
        let mut diag_rx = handle.diagnostics().subscribe();

        // Send 257 events to exceed the buffer capacity of 256
        for i in 0..257 {
            let _ = handle
                .diag_tx
                .send(crate::diagnostics::DiagnosticEvent::PipelinePhase {
                    turn: TurnId::new(),
                    phase: format!("phase {i}"),
                });
        }

        // Try to receive and it should return RecvError::Lagged
        let res = diag_rx.recv().await;
        assert!(
            matches!(
                res,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
            ),
            "Expected RecvError::Lagged for diagnostics channel overflow, got {res:?}"
        );

        let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
    }
}
