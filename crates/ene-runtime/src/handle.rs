#[cfg(any(unix, windows))]
use crate::db_server::DbIpcServer;
use crate::diagnostics::{DiagnosticEvent, MemoryQueryHandle, emit_diag};
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
    compression_has_usable_summary,
};
use ene_mind::{ConversationSession, EneSessionError, SplitResult};
use ene_tool_host::{ToolHostManager, ToolRegistry};
use ene_tool_proto::ToolSpec;
use ene_tool_rag::{ToolRag, ToolRagConfig, ToolRagOptions};
use std::collections::HashMap;
/// Global monotonic counter used to generate unique DB IPC auth tokens.
/// Intentionally process-global: each `EneHandle::open` call increments
/// the counter so concurrent handles never share a token.
static DB_TOKEN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A deferred (background) tool task tracked by the actor (#196).
///
/// Created when a background-capable tool accepts a deferred call and
/// returns a `task_id`. The actor polls the owning tool until the task
/// reaches a terminal state, then emits [`EneEvent::ToolBackgroundCompleted`].
#[derive(Debug, Clone)]
pub struct DeferredToolTask {
    /// The tool name that owns the background task.
    pub tool_name: String,
    /// The `task_id` returned by the deferred call acceptance.
    pub task_id: String,
    /// JSON-encoded arguments the task was started with.
    pub arguments: String,
    /// When the task was accepted for background execution.
    pub started_at: DateTime<Utc>,
}

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
    /// List all session-wide permission grants (#177).
    ListPermissions {
        /// Reply channel for the granted scopes.
        reply: oneshot::Sender<Vec<crate::streaming::PermissionScope>>,
    },
    /// Revoke a single session-wide permission grant by id (#177).
    RevokePermission {
        /// The `PermissionScope::id` to revoke.
        id: u64,
        /// Reply channel reporting whether a scope was removed.
        reply: oneshot::Sender<bool>,
    },
    /// Revoke all session-wide permission grants (#177).
    ResetAllPermissions {
        /// Reply channel carrying the number of revoked scopes.
        reply: oneshot::Sender<usize>,
    },
    /// Undo the most recent reversible tool operation (#178).
    Undo {
        /// Reply channel carrying the undo report.
        reply: oneshot::Sender<crate::undo::UndoReport>,
    },
    /// List stored sessions, newest first (#176).
    ListSessions {
        /// Whether to include archived sessions.
        include_archived: bool,
        /// Maximum number of sessions to return.
        limit: usize,
        /// Reply channel carrying the session list.
        reply: oneshot::Sender<Result<Vec<ene_store::SessionMeta>, ene_store::EneMemoryError>>,
    },
    /// Export a session to a versioned, redacted JSON bundle (#176).
    ExportSession {
        /// Logical session id to export.
        session_id: String,
        /// Reply channel carrying the JSON export string.
        reply: oneshot::Sender<Result<String, ene_store::EneMemoryError>>,
    },
    /// Import a session from a JSON export bundle (#176).
    ImportSession {
        /// JSON export bundle to import.
        json: String,
        /// Reply channel carrying the imported session row id.
        reply: oneshot::Sender<Result<i64, ene_store::EneMemoryError>>,
    },
    /// Full-text search over stored conversation messages (#176).
    SearchSessions {
        /// Case-insensitive search query.
        query: String,
        /// Maximum number of matches to return.
        limit: usize,
        /// Number of matches to skip (pagination).
        offset: usize,
        /// Reply channel carrying `(session_id, message)` matches.
        reply: oneshot::Sender<
            Result<Vec<(String, ene_store::ExportedMessage)>, ene_store::EneMemoryError>,
        >,
    },
    /// Archive or unarchive a session (#176).
    ArchiveSession {
        /// Logical session id to update.
        session_id: String,
        /// Whether the session should be archived.
        archived: bool,
        /// Reply channel carrying whether a session row was updated.
        reply: oneshot::Sender<Result<bool, ene_store::EneMemoryError>>,
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
    /// Search tools in the active tool registry using RAG if available.
    SearchTools {
        /// The query to search for.
        query: String,
        /// Reply channel for the matching tools.
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
    /// Cancel a deferred (background) tool task by id (#196).
    ///
    /// Routes to the owning tool and asks it to abort the background task.
    /// The reply reports whether a running task was actually cancelled.
    CancelDeferredTool {
        /// The tool name that owns the background task.
        tool_name: String,
        /// The `task_id` returned by the deferred call acceptance.
        task_id: String,
        /// Reply channel carrying whether a running task was cancelled.
        reply: oneshot::Sender<bool>,
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
        /// Privacy-safe OS app label (may be empty).
        app_label: String,
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
    /// A deferred (background) tool task has reached a terminal state (#196).
    ///
    /// Emitted asynchronously after the originating turn has completed, once
    /// the background-capable tool reports that the task finished, failed, or
    /// was cancelled. Consumers can use `task_id` to correlate this with the
    /// earlier `DeferredAccepted` result returned to the LLM.
    ToolBackgroundCompleted {
        /// The tool name that owns the background task.
        tool_name: String,
        /// The `task_id` returned by the deferred call acceptance.
        task_id: String,
        /// Terminal status of the background task.
        status: ene_tool_proto::DeferredStatus,
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
/// consuming events from the actor. On lag, emits
/// [`DiagnosticEvent::Lagged`] / [`DiagnosticEvent::ResyncNeeded`] so gaps
/// are never silent (#189).
pub struct EneEventReceiver {
    inner: broadcast::Receiver<EneEvent>,
    diag_tx: broadcast::Sender<crate::diagnostics::DiagnosticEvent>,
}

impl std::fmt::Debug for EneEventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EneEventReceiver").finish()
    }
}

impl EneEventReceiver {
    fn note_lag(&self, skipped: u64) {
        let _ = self
            .diag_tx
            .send(crate::diagnostics::DiagnosticEvent::Lagged {
                channel: "events".to_string(),
                skipped,
            });
        let _ = self
            .diag_tx
            .send(crate::diagnostics::DiagnosticEvent::ResyncNeeded {
                channel: "events".to_string(),
            });
    }

    /// Non-blocking poll of the event stream.
    pub fn try_recv(&mut self) -> Result<EneEvent, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::TryRecvError::Lagged(n))
            }
            other => other,
        }
    }

    /// Async receive, waiting for the next event.
    pub async fn recv(&mut self) -> Result<EneEvent, broadcast::error::RecvError> {
        match self.inner.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::RecvError::Lagged(n))
            }
            other => other,
        }
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
                health_monitor: self.diagnostics.health_monitor.clone(),
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

        let mind = config.get_section::<ene_mind::MindConfig>()?;
        ene_mind::CognitionEngine::validate_config(&mind).map_err(EneRuntimeError::from)?;

        let mem_config = config.get_section::<ene_store::StoreConfig>()?;
        let tool_config = config.get_section::<ene_tool_host::ToolConfig>()?;
        let rag_config = config.get_section::<ToolRagConfig>()?;
        let needs_embedder = mem_config.enabled || (tool_config.enabled && rag_config.enabled);

        // Prefetch configured GGUF weights in parallel before backends load them.
        {
            let ai_config = config.get_section::<ene_ai::AiConfig>()?;
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
                    EneRuntimeError::Memory(ene_store::EneMemoryError::MemoryStoreConnectionError(
                        e,
                    ))
                })?;
            session.memory.memory_store = Some(store.clone());
            Some(store)
        } else {
            None
        };

        let registry = build_tool_registry(&config, memory_store.clone(), diag_tx.clone()).await?;
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

        let mind_memory = mind.memory.clone();
        let memory = MemoryQueryHandle::new(
            session.memory.memory_store.clone(),
            session.memory.embedding_provider.clone(),
            mind_memory,
        );

        // Provider health monitor for failover diagnostics (#175).
        let fallback_cfg = config.get_section::<ene_ai::AiConfig>()?.fallback;
        let health_monitor = ene_ai::ProviderHealthMonitor::new(
            std::time::Duration::from_millis(fallback_cfg.cache_ttl_ms),
            fallback_cfg.max_history,
        );

        let diagnostics = crate::diagnostics::EneDiagnostics {
            cmd_tx: Arc::clone(&cmd_tx),
            diag_tx: diag_tx.clone(),
            memory,
            health_monitor: health_monitor.clone(),
        };

        let turn_gate = Arc::new(TurnGate::new());
        let (classifier_tx, classifier_rx) = mpsc::unbounded_channel();
        let (memory_writer_tx, memory_writer_rx) = mpsc::unbounded_channel();
        let (deferred_tool_tx, deferred_tool_rx) = mpsc::unbounded_channel();

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
            permission_scopes: Arc::new(Mutex::new(Vec::new())),
            undo_stack: Arc::new(Mutex::new(crate::undo::UndoStack::new(64))),
            context: ene_mind::ContextManager::default(),
            call_tool_tasks: tokio::task::JoinSet::new(),
            classifier_tasks: tokio::task::JoinSet::new(),
            memory_writer_tasks: tokio::task::JoinSet::new(),
            vision_tasks: tokio::task::JoinSet::new(),
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
        EneEventReceiver {
            inner: self.event_tx.subscribe(),
            diag_tx: self.diag_tx.clone(),
        }
    }

    /// Concrete diagnostics facade (pipeline detail, memory, tools).
    pub const fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics {
        &self.diagnostics
    }

    /// Start a turn. Returns [`RunError::Busy`] if a turn is already in flight
    /// — never silently aborts the active turn.
    #[must_use = "the returned TurnId is needed for cancellation"]
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
    #[must_use = "caller should check whether cancellation succeeded"]
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

    /// List all session-wide permission grants (#177).
    pub async fn list_permissions(
        &self,
    ) -> Result<Vec<crate::streaming::PermissionScope>, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListPermissions { reply })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Revoke a single session-wide permission grant by id (#177).
    pub async fn revoke_permission(&self, id: u64) -> Result<bool, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::RevokePermission { id, reply })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Revoke all session-wide permission grants (#177).
    pub async fn reset_all_permissions(&self) -> Result<usize, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ResetAllPermissions { reply })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Undo the most recent reversible tool operation (#178).
    pub async fn undo(&self) -> Result<crate::undo::UndoReport, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::Undo { reply })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// List stored sessions, newest first (#176).
    pub async fn list_sessions(
        &self,
        include_archived: bool,
        limit: usize,
    ) -> Result<Result<Vec<ene_store::SessionMeta>, ene_store::EneMemoryError>, ActorDeadError>
    {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListSessions {
                include_archived,
                limit,
                reply,
            })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Export a session to a versioned, redacted JSON bundle (#176).
    pub async fn export_session(
        &self,
        session_id: impl Into<String>,
    ) -> Result<Result<String, ene_store::EneMemoryError>, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ExportSession {
                session_id: session_id.into(),
                reply,
            })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Import a session from a JSON export bundle (#176).
    pub async fn import_session(
        &self,
        json: impl Into<String>,
    ) -> Result<Result<i64, ene_store::EneMemoryError>, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ImportSession {
                json: json.into(),
                reply,
            })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Full-text search over stored conversation messages (#176).
    pub async fn search_sessions(
        &self,
        query: impl Into<String>,
        limit: usize,
        offset: usize,
    ) -> Result<
        Result<Vec<(String, ene_store::ExportedMessage)>, ene_store::EneMemoryError>,
        ActorDeadError,
    > {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SearchSessions {
                query: query.into(),
                limit,
                offset,
                reply,
            })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
    }

    /// Archive or unarchive a session (#176).
    pub async fn archive_session(
        &self,
        session_id: impl Into<String>,
        archived: bool,
    ) -> Result<Result<bool, ene_store::EneMemoryError>, ActorDeadError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ArchiveSession {
                session_id: session_id.into(),
                archived,
                reply,
            })
            .map_err(|_| ActorDeadError)?;
        rx.await.map_err(|_| ActorDeadError)
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
        settings: FeatureSettingsUpdate,
    ) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::UpdateFeatureSettings {
                settings: Box::new(settings),
            })
            .map_err(|_| ActorDeadError)
    }

    /// Summarize a screen RGB capture via the local vision model (Gemma + mmproj).
    pub async fn summarize_screen_image(
        &self,
        width: u32,
        height: u32,
        rgb: Vec<u8>,
        app_label: String,
    ) -> Result<String, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SummarizeScreenImage {
                width,
                height,
                rgb,
                app_label,
                reply: tx,
            })
            .map_err(|_| "actor dead".to_string())?;
        rx.await.map_err(|_| "actor dropped reply".to_string())?
    }
}

impl Drop for EneHandle {
    /// Sends a graceful `Shutdown` command when the last handle clone is dropped.
    ///
    /// Because `Drop` is synchronous, the actor may still be running after this
    /// returns. Background tasks spawned by the actor (classifiers, memory
    /// writers, deferred tool tasks) become detached tokio tasks. Callers that
    /// need a clean shutdown guarantee should use [`EneHandle::shutdown`] with
    /// an explicit timeout before dropping the handle.
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
    /// Session-wide permission grants tracked for the permission center (#177).
    permission_scopes: Arc<Mutex<Vec<crate::streaming::PermissionScope>>>,
    /// Actor-native undo stack of mutating tool calls (#178).
    undo_stack: Arc<Mutex<crate::undo::UndoStack>>,
    context: ene_mind::ContextManager,
    call_tool_tasks: tokio::task::JoinSet<()>,
    classifier_tasks: tokio::task::JoinSet<()>,
    memory_writer_tasks: tokio::task::JoinSet<()>,
    /// In-flight screen-summary vision jobs (must not block the command loop).
    vision_tasks: tokio::task::JoinSet<()>,
    /// In-flight tool-search jobs; reaped so panics are not lost (#236).
    search_tasks: tokio::task::JoinSet<()>,
    /// In-flight deferred (background) tool tasks (#196). Each task polls
    /// its owning tool until the task reaches a terminal state, then emits
    /// [`EneEvent::ToolBackgroundCompleted`]. Reaped so panics are not lost.
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
    /// Provider health monitor for failover routing (#175).
    health_monitor: ene_ai::ProviderHealthMonitor,
    /// Maximum poll iterations for deferred (background) tool tasks (#196).
    /// Configurable via `ENE_TOOLS__DEFERRED_MAX_POLLS` env var (default: 600 = 60s).
    deferred_max_polls: u32,
}

/// Runs a future to completion, catching any panic and surfacing it as a
/// [`DiagnosticEvent::ActorPanic`] instead of unwinding the caller (#236).
///
/// Returns `Ok(output)` on normal completion, or `Err(message)` when the
/// future panicked. This keeps the actor command loop (and any other
/// supervisor site) alive across a panicking unit of work.
async fn isolate_panic<F, T>(
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
            let _ = diag_tx.send(DiagnosticEvent::ActorPanic {
                component: component.to_string(),
                message: message.clone(),
            });
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
                let _ = diag_tx.send(DiagnosticEvent::ActorPanic {
                    component: component.to_string(),
                    message: e.to_string(),
                });
            }
        }
    }
}

/// Polls a deferred (background) tool task until it reaches a terminal state (#196).
///
/// Emits [`EneEvent::ToolBackgroundCompleted`] when the task completes, fails,
/// or is cancelled. Runs as a background task in the actor's `deferred_tool_tasks`
/// `JoinSet`.
///
/// `max_polls` controls how many poll iterations (at 100ms each) before the task
/// is considered timed out. Override via the `ENE_TOOLS__DEFERRED_MAX_POLLS` env var
/// (default: 600 = 60s).
async fn poll_deferred_task(
    registry: Arc<dyn ToolRegistry>,
    event_tx: broadcast::Sender<EneEvent>,
    task: DeferredToolTask,
    max_polls: u32,
) {
    use ene_tool_proto::DeferredStatus;
    use std::time::Duration;

    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    for _ in 0..max_polls {
        let status = registry.poll_deferred(&task.tool_name, &task.task_id).await;
        match status {
            DeferredStatus::Pending => {
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            DeferredStatus::Completed { result } => {
                let _ = event_tx.send(EneEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Completed { result },
                });
                return;
            }
            DeferredStatus::Failed { error } => {
                let _ = event_tx.send(EneEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Failed { error },
                });
                return;
            }
            DeferredStatus::Cancelled => {
                let _ = event_tx.send(EneEvent::ToolBackgroundCompleted {
                    tool_name: task.tool_name.clone(),
                    task_id: task.task_id.clone(),
                    status: DeferredStatus::Cancelled,
                });
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

impl EneActor {
    /// Runs a single command, isolating any panic so the actor loop survives (#236).
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

    async fn run(mut self) {
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
                &mut self.vision_tasks,
                "VisionReaper",
                "Screen summary vision task panicked",
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
                let store = self.session.memory.memory_store.clone();
                self.memory_writer_tasks.spawn(async move {
                    match handle.await {
                        Ok(ene_mind::MemoryWriteOutcome::Ok) => {}
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
                            let _ = diag_tx.send(DiagnosticEvent::MemoryWrite {
                                character_id,
                                status: status.to_string(),
                                message,
                                pending_id,
                                pending_count,
                                permanent_count,
                            });
                        }
                        Err(e) => {
                            tracing::error!(
                                component = "MemoryWriter",
                                error = %e,
                                "Deferred memory-writer task panicked"
                            );
                            let _ = diag_tx.send(DiagnosticEvent::MemoryWrite {
                                character_id: String::new(),
                                status: "failed".to_string(),
                                message: format!("memory writer task panicked: {e}"),
                                pending_id: None,
                                pending_count: None,
                                permanent_count: None,
                            });
                        }
                    }
                });
            }

            // Drain deferred tool tasks accepted by the stream (#196).
            // Spawn a polling task for each that awaits completion.
            while let Ok(task) = self.deferred_tool_rx.try_recv() {
                let registry = Arc::clone(&self.registry);
                let event_tx = self.event_tx.clone();
                let max_polls = self.deferred_max_polls;
                self.deferred_tool_tasks.spawn(async move {
                    poll_deferred_task(registry, event_tx, task, max_polls).await;
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
        self.vision_tasks.abort_all();
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
        app_label: String,
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

        match crate::proactive::rgb_to_jpeg_data_uri(width, height, &rgb) {
            Ok(uri) => {
                self.proactive.last_screen_image_data_uri = Some(uri);
            }
            Err(e) => {
                tracing::warn!(
                    component = "Proactive",
                    error = %e,
                    "Failed to stash screen frame for generation; continuing text-only"
                );
                self.proactive.last_screen_image_data_uri = None;
            }
        }

        let prompts = ene_config::PromptLibrary::load(&prompt_language);
        let system = prompts.proactive().screen_summary_system.trim().to_string();
        let user = prompts.proactive().render_screen_summary_user(&app_label);
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
                        self.diag_tx.clone(),
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
                app_label,
                reply,
            } => {
                self.summarize_screen_rgb(width, height, rgb, app_label, reply)
                    .await;
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
            EneCommand::ListPermissions { reply } => {
                let scopes = self.permission_scopes.lock().await.clone();
                let _ = reply.send(scopes);
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
                let _ = reply.send(count);
                true
            }
            EneCommand::Undo { reply } => {
                let report = self.handle_undo().await;
                let _ = reply.send(report);
                true
            }
            EneCommand::ListSessions {
                include_archived,
                limit,
                reply,
            } => {
                let result = match self.session.memory.memory_store.as_ref() {
                    Some(store) => store.list_sessions(include_archived, limit).await,
                    None => Err(ene_store::EneMemoryError::Other(
                        "Memory store is not enabled".to_string(),
                    )),
                };
                let _ = reply.send(result);
                true
            }
            EneCommand::ExportSession { session_id, reply } => {
                let result = match self.session.memory.memory_store.as_ref() {
                    Some(store) => match store.build_export(&session_id).await {
                        Ok(export) => export.to_json(),
                        Err(e) => Err(e),
                    },
                    None => Err(ene_store::EneMemoryError::Other(
                        "Memory store is not enabled".to_string(),
                    )),
                };
                let _ = reply.send(result);
                true
            }
            EneCommand::ImportSession { json, reply } => {
                let result = match self.session.memory.memory_store.as_ref() {
                    Some(store) => match ene_store::SessionExport::from_json(&json) {
                        Ok(export) => store.import_export(&export).await,
                        Err(e) => Err(e),
                    },
                    None => Err(ene_store::EneMemoryError::Other(
                        "Memory store is not enabled".to_string(),
                    )),
                };
                let _ = reply.send(result);
                true
            }
            EneCommand::SearchSessions {
                query,
                limit,
                offset,
                reply,
            } => {
                let result = match self.session.memory.memory_store.as_ref() {
                    Some(store) => store.search_messages(&query, limit, offset).await,
                    None => Err(ene_store::EneMemoryError::Other(
                        "Memory store is not enabled".to_string(),
                    )),
                };
                let _ = reply.send(result);
                true
            }
            EneCommand::ArchiveSession {
                session_id,
                archived,
                reply,
            } => {
                let result = match self.session.memory.memory_store.as_ref() {
                    Some(store) => store.set_session_archived(&session_id, archived).await,
                    None => Err(ene_store::EneMemoryError::Other(
                        "Memory store is not enabled".to_string(),
                    )),
                };
                let _ = reply.send(result);
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
                let mut tools = self.registry.list_tools();
                tools.push(crate::streaming::search_tools_spec());
                let _ = reply.send(tools);
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
                    let _ = reply.send(result);
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
                    if let Some(ref turn) = turn {
                        let call_ctx = ene_tool_proto::CallContext {
                            conversation_id: session_id,
                            turn_id: turn.to_string(),
                        };
                        registry.set_call_context(&call_ctx).await;
                    }
                    let result = if name == "system.search_tools" {
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
                            .call_tool(&name, &arguments)
                            .await
                            .map_err(EneRuntimeError::from)
                    };
                    let _ = reply.send(result);
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
                    // from "already finished" without a follow-up poll.
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
                permission_scopes,
                undo_stack,
                terminal_emitted,
                turn: turn_for_stream,
                origin,
                allow_tools,
                runtime_directive,
                proactive_screen_image,
                generation_timeout,
                classifier_tx,
                memory_writer_tx,
                deferred_tool_tx,
            })
            .await;
            let _ = session_tx.send(outcome);
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
            let _ = self.diag_tx.send(DiagnosticEvent::ProviderHealth {
                provider: report.provider.clone(),
                status: report.status.status_code().to_string(),
                latency_ms: report.latency_ms,
                detail: report.error.clone(),
            });
        }

        if selection.fell_back {
            let reason = selection
                .skipped
                .iter()
                .map(|(p, r)| format!("{p}: {r}"))
                .collect::<Vec<_>>()
                .join("; ");
            let _ = self.diag_tx.send(DiagnosticEvent::ProviderFallback {
                from: candidates
                    .first()
                    .map_or_else(String::new, |c| c.provider.clone()),
                to: selection.candidate.provider.clone(),
                reason,
            });
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

        match self.registry.call_tool("utility.undo", "{}").await {
            Ok(output) => UndoReport::Reverted {
                metadata: entry.metadata,
                output,
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
///
/// Tool health events (#238) are bridged into `diag_tx` as
/// [`DiagnosticEvent::ToolHealth`].
async fn build_tool_registry(
    config: &EneConfig,
    memory_store: Option<Arc<ene_store::MemoryStore>>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
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

    let (health_tx, mut health_rx) =
        tokio::sync::mpsc::unbounded_channel::<ene_tool_host::ToolHealthEvent>();

    let registry = ToolHostManager::start_full(config, db_tokens, Some(health_tx))
        .await
        .map_err(EneRuntimeError::Tool)?;

    // Bridge tool health events into the diagnostics channel (#238).
    tokio::spawn(async move {
        while let Some(event) = health_rx.recv().await {
            emit_diag(&diag_tx, tool_health_event_to_diag(event));
        }
    });

    Ok(registry)
}

/// Maps a [`ene_tool_host::ToolHealthEvent`] to a [`DiagnosticEvent::ToolHealth`]
/// with a stable English status contract (#238).
fn tool_health_event_to_diag(event: ene_tool_host::ToolHealthEvent) -> DiagnosticEvent {
    use ene_tool_host::ToolHealthEvent;
    let (tool, status, detail) = match event {
        ToolHealthEvent::Unhealthy { tool, reason } => {
            (tool, "unhealthy", Some(format!("tool is {reason}")))
        }
        ToolHealthEvent::Restarting { tool, attempt } => (
            tool,
            "restarting",
            Some(format!("restart attempt {attempt}")),
        ),
        ToolHealthEvent::Restarted { tool } => (tool, "restarted", None),
        ToolHealthEvent::Recovered { tool } => (tool, "recovered", None),
        ToolHealthEvent::CircuitOpened {
            tool,
            consecutive_failures,
        } => (
            tool,
            "circuit_open",
            Some(format!("{consecutive_failures} consecutive failures")),
        ),
        ToolHealthEvent::CircuitClosed { tool } => (tool, "circuit_closed", None),
        ToolHealthEvent::Disabled { tool } => (
            tool,
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

async fn init_memory_store(
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
fn init_tool_rag(
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
        let mut diag_rx = handle.diagnostics().subscribe();

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

        let mut saw_lagged = false;
        let mut saw_resync = false;
        while let Ok(ev) = diag_rx.try_recv() {
            match ev {
                crate::diagnostics::DiagnosticEvent::Lagged { channel, .. }
                    if channel == "events" =>
                {
                    saw_lagged = true;
                }
                crate::diagnostics::DiagnosticEvent::ResyncNeeded { channel }
                    if channel == "events" =>
                {
                    saw_resync = true;
                }
                _ => {}
            }
        }
        assert!(
            saw_lagged,
            "expected DiagnosticEvent::Lagged after event lag"
        );
        assert!(
            saw_resync,
            "expected DiagnosticEvent::ResyncNeeded after event lag"
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

    #[tokio::test]
    async fn isolate_panic_returns_output_on_success() {
        let (diag_tx, _diag_rx) = broadcast::channel(16);
        let result = isolate_panic(&diag_tx, "test", async { 42u32 }).await;
        assert_eq!(result.expect("no panic"), 42);
    }

    #[tokio::test]
    async fn isolate_panic_contains_panic_and_emits_diagnostic() {
        let (diag_tx, mut diag_rx) = broadcast::channel(16);
        let result: Result<u32, String> = isolate_panic(&diag_tx, "test-component", async {
            panic!("boom");
        })
        .await;
        let message = result.expect_err("panic must be contained, not propagated");
        assert!(message.contains("boom"), "unexpected message: {message}");

        // The supervisor must surface the panic as a diagnostic event.
        let mut saw_panic = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let crate::diagnostics::DiagnosticEvent::ActorPanic { component, .. } = ev {
                assert_eq!(component, "test-component");
                saw_panic = true;
            }
        }
        assert!(saw_panic, "expected ActorPanic diagnostic event");
    }
}
