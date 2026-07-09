#[cfg(any(unix, windows))]
use crate::db_server::DbIpcServer;
use crate::error::EneCoreError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::RequestId;
use chrono::{DateTime, Utc};
use ene_cognition::{
    CompressionLevel, CompressionTaskInput, HistoryEntry as CognitionHistoryEntry,
    MIN_MESSAGES_TO_COMPRESS, PendingCompressionTask, compression_has_usable_summary,
    evaluate_compression_trigger, execute_compression, maybe_roll_up_chapter,
    poll_compression_result, spawn_compression_task,
};
use ene_config::CharacterCardV3;
use ene_config::EneConfig;
use ene_provider::LlmProviderRegistry;
use ene_provider::Role;
use ene_session::PendingSplitTask;
use ene_session::{CardName, SessionId};
use ene_session::{
    ConversationSession, EneSessionError, SplitReason, SplitResult, poll_split_result,
};
use ene_tool_host::{CompositeToolRegistry, ToolHostManager, ToolRegistry};
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
    },
    /// Cancel the currently-running AI completion stream.
    Cancel,
    /// Shut down the actor and clean up background tasks.
    Shutdown,
    /// Apply new configuration and re-initialize subsystems (tools, memory, embedding).
    /// The result is returned through the oneshot channel.
    Reconfigure {
        /// The replacement configuration to apply.
        config: EneConfig,
        /// Confirmation channel; the actor sends `Ok(())` or an `EneCoreError`.
        reply: oneshot::Sender<Result<(), EneCoreError>>,
    },
    /// Load a character card from the given path.
    LoadCharacter {
        /// Path to the character card (`.json` or `.png`).
        path: String,
        /// Confirmation channel.
        reply: oneshot::Sender<Result<(), EneCoreError>>,
    },
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
    /// The summary and new session ID are returned through the oneshot channel.
    ManualSplit {
        /// Result channel carrying the split result or an error.
        reply: oneshot::Sender<Result<SplitResult, EneCoreError>>,
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
        reply: oneshot::Sender<Result<String, EneCoreError>>,
    },
    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    InvalidateToolIndex,
}

/// Events emitted from the actor to all consumers via broadcast channel.
///
/// Consumers (CLI, Bevy systems, logging) receive these through
/// [`EneHandle::subscribe`] which returns an [`EneEventReceiver`].
#[derive(Debug, Clone)]
pub enum EneEvent {
    /// A chunk of generated text from the LLM.
    TextDelta {
        /// The raw text delta.
        delta: String,
    },
    /// A special token like `<|emo:happy|>` already parsed from the stream.
    SpecialToken {
        /// The full token string (e.g. `<|emo:happy|>`).
        token: String,
    },
    /// Engine-managed expression resolved by the Output Arbiter (#91).
    Expression {
        /// Normalized expression name (e.g. `happy`, `neutral`).
        name: String,
        /// Resolution source for debugging (`affect`, `llm_advisory`, `hysteresis`, …).
        source: String,
    },
    /// A tool call has been requested by the LLM.
    ToolCallStart {
        /// The tool name (e.g. "fs.write").
        name: String,
        /// JSON-encoded arguments.
        arguments: String,
    },
    /// A tool call has completed with its result.
    ToolCallResult {
        /// The tool name.
        name: String,
        /// The tool's output as a string.
        result: String,
    },
    /// A destructive operation requires user approval before execution.
    PermissionRequired {
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
    /// The consumer should display a UI and call [`EneHandle::submit_user_input`].
    UserInputRequired {
        /// Unique identifier for this input request.
        request_id: RequestId,
        /// The prompt describing the question, options, and free-text allowance.
        prompt: ene_tool_proto::UserInputPrompt,
    },
    /// Progress update for a long-running background task.
    TaskProgress {
        /// Unique task identifier.
        task_id: String,
        /// Current step number.
        step: usize,
        /// Total number of steps (`None` if unknown).
        total_steps: Option<usize>,
        /// Description of the current step.
        description: String,
    },
    /// Status update indicating the current pipeline phase.
    PipelinePhase {
        /// Short description of the current phase.
        phase: String,
    },
    /// Emitted just before the first TextDelta, summarizing the execution time of pre-generation phases.
    PipelineMetrics {
        /// Map of phase names to elapsed time in milliseconds.
        timings: std::collections::HashMap<String, u64>,
    },
    /// The conversation session has been split (timeout, topic change, or manual).
    SessionSplit {
        /// Generated summary of the conversation segment.
        summary: String,
        /// Why the split was triggered.
        reason: SplitReason,
    },
    /// Terminal event for a run: emitted exactly once at the end of every
    /// `EneCommand::Run`, whether the stream finished normally, errored out,
    /// or was cancelled by the user. Consumers should break their event
    /// loop on this variant.
    Terminal(
        /// Why the run terminated.
        TerminalReason,
    ),
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

/// A read-only handle for querying the memory subsystem.
///
/// Wraps the memory store and embedding provider, exposing only
/// the operations needed by downstream consumers (CLI commands, etc.).
#[derive(Clone)]
pub struct MemoryQueryHandle {
    store: Option<Arc<ene_memory::MemoryStore>>,
    embedder: Option<Arc<dyn ene_provider::EmbeddingProvider>>,
}

impl std::fmt::Debug for MemoryQueryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryQueryHandle")
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl MemoryQueryHandle {
    /// Whether memory is enabled and both store and embedder are available.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.store.is_some() && self.embedder.is_some()
    }

    /// Embed a text query for similarity search.
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EneCoreError> {
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            EneCoreError::Embedding(ene_provider::EmbeddingError::Init(
                "Embedding provider not available".into(),
            ))
        })?;
        embedder.embed_query(text).await.map_err(EneCoreError::from)
    }

    /// Search conversation summaries by embedding similarity.
    pub async fn search_summaries(
        &self,
        query_embedding: &[f32],
        card_name: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<ene_memory::RecalledSummary>, EneCoreError> {
        let store = self.store.as_ref().ok_or_else(|| {
            EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(
                "Memory store not available".into(),
            ))
        })?;
        store
            .search_summaries(query_embedding, card_name, limit, threshold)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// List recent conversation summaries for a character card.
    pub async fn list_recent_summaries(
        &self,
        card_name: &str,
        limit: usize,
    ) -> Result<Vec<ene_memory::ConversationSummary>, EneCoreError> {
        let store = self.store.as_ref().ok_or_else(|| {
            EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(
                "Memory store not available".into(),
            ))
        })?;
        store
            .list_recent_summaries(card_name, limit)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// List all known key facts for a character card.
    pub async fn get_all_keyfacts(
        &self,
        card_name: &str,
    ) -> Result<Vec<ene_memory::KeyFact>, EneCoreError> {
        let store = self.store.as_ref().ok_or_else(|| {
            EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(
                "Memory store not available".into(),
            ))
        })?;
        store
            .get_all_keyfacts(card_name)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Count legacy memory rows for a character card.
    pub async fn count_legacy_rows(
        &self,
        card_name: &str,
    ) -> Result<ene_memory::LegacyRowCounts, EneCoreError> {
        let store = self.require_store()?;
        store
            .count_legacy_rows(card_name)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Migration status for a character card.
    pub async fn migration_status(
        &self,
        card_name: &str,
    ) -> Result<Option<ene_memory::MigrationStatus>, EneCoreError> {
        let store = self.require_store()?;
        store
            .get_migration_status(card_name)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Run legacy → typed one-shot migration.
    pub async fn migrate_legacy(
        &self,
        card_name: &str,
        user_id: &str,
        dry_run: bool,
    ) -> Result<ene_memory::LegacyMigrationReport, EneCoreError> {
        let store = self.require_store()?;
        let model = self
            .embedder
            .as_ref()
            .ok_or_else(|| {
                EneCoreError::Embedding(ene_provider::EmbeddingError::Init(
                    "Embedding provider not available".into(),
                ))
            })?
            .model_name()
            .to_string();
        let options = ene_memory::LegacyMigrationOptions {
            card_name: card_name.to_string(),
            user_id: user_id.to_string(),
            embedding_model: model,
            dry_run,
        };
        store
            .migrate_legacy(&options)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Destructive legacy memory reset for a character card.
    pub async fn reset_legacy_memory(&self, card_name: &str) -> Result<(), EneCoreError> {
        let store = self.require_store()?;
        store
            .reset_legacy_memory(card_name)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// List typed memories for the current character.
    pub async fn list_typed_memories(
        &self,
        character_id: &str,
        kind: Option<ene_memory::MemoryKind>,
        limit: usize,
    ) -> Result<Vec<ene_memory::MemoryItem>, EneCoreError> {
        let store = self.require_store()?;
        store
            .get_typed_memories_by_character(character_id, kind, limit, 0)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// List typed memories for the memory journal with user/scope and status filters.
    pub async fn list_journal_memories(
        &self,
        options: &ene_memory::MemoryJournalListOptions<'_>,
    ) -> Result<Vec<ene_memory::MemoryItem>, EneCoreError> {
        let store = self.require_store()?;
        store
            .list_journal_memories(options)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Fetch a typed memory by id.
    pub async fn inspect_typed_memory(
        &self,
        id: i64,
    ) -> Result<Option<ene_memory::MemoryItem>, EneCoreError> {
        let store = self.require_store()?;
        store
            .get_typed_memory(id)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Search typed memories using hybrid scoring.
    pub async fn search_typed_memories_hybrid(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ene_memory::ScoredMemory>, EneCoreError> {
        let store = self.require_store()?;
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            EneCoreError::Embedding(ene_provider::EmbeddingError::Init(
                "Embedding provider not available".into(),
            ))
        })?;
        let query_embedding = embedder.embed_query(query_text).await?;
        let options = ene_memory::MemorySearchOptions {
            query_text,
            query_embedding: &query_embedding,
            character_id,
            user_id,
            model_name: embedder.model_name(),
            limit,
            similarity_threshold: 0.45,
            candidate_pool_size: 64,
            query_affect: None,
            weights: ene_memory::HybridSearchWeights::default(),
            decay_half_life_days: 30.0,
            now: chrono::Utc::now(),
            min_score: 0.10,
            commitment_boost: 0.25,
            recent_fallback_limit: 6,
        };
        store
            .search_typed_memories_hybrid(&options)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Search typed memories and attach explainable recall reasons (#74).
    pub async fn search_typed_memories_explained(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ene_cognition::RecalledMemory>, EneCoreError> {
        let scored = self
            .search_typed_memories_hybrid(character_id, user_id, query_text, limit)
            .await?;
        Ok(ene_cognition::explain_scored_memories(scored))
    }

    /// Update typed memory pinned flag.
    pub async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, EneCoreError> {
        let store = self.require_store()?;
        store
            .pin_typed_memory(id, pinned)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Transition typed memory lifecycle status.
    pub async fn transition_typed_memory_status(
        &self,
        id: i64,
        status: ene_memory::MemoryStatus,
    ) -> Result<bool, EneCoreError> {
        let store = self.require_store()?;
        store
            .transition_typed_memory_status(id, status)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// User-driven restore to active status (journal/CLI UX).
    pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, EneCoreError> {
        let store = self.require_store()?;
        store
            .user_restore_typed_memory(id)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// User-driven forget (`Active` → `UserDeleted`).
    pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, EneCoreError> {
        let store = self.require_store()?;
        store
            .user_forget_typed_memory(id)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Show current affect state for a character.
    pub async fn show_affect_state(
        &self,
        character_id: &str,
    ) -> Result<ene_memory::AffectState, EneCoreError> {
        let store = self.require_store()?;
        store
            .get_affect_state(character_id)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Reset affect state to neutral baseline.
    pub async fn reset_affect_state(&self, character_id: &str) -> Result<(), EneCoreError> {
        let store = self.require_store()?;
        let neutral = ene_memory::AffectState::neutral(character_id);
        store
            .upsert_affect_state(&neutral)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// List active commitments for a character/user.
    pub async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ene_memory::Commitment>, EneCoreError> {
        let store = self.require_store()?;
        store
            .list_active_commitments(character_id, user_id, limit)
            .await
            .map_err(EneCoreError::Memory)
    }

    /// Mark a commitment as done.
    pub async fn complete_commitment(&self, id: i64) -> Result<bool, EneCoreError> {
        let store = self.require_store()?;
        store
            .complete_commitment(id)
            .await
            .map_err(EneCoreError::Memory)
    }

    fn require_store(&self) -> Result<&std::sync::Arc<ene_memory::MemoryStore>, EneCoreError> {
        self.store.as_ref().ok_or_else(|| {
            EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(
                "Memory store not available".into(),
            ))
        })
    }
}

/// A snapshot of the current actor state for read-only queries.
#[derive(Clone)]
pub struct EneStateSnapshot {
    /// The loaded character card, if any.
    pub character_card: Option<CharacterCardV3>,
    /// Conversation history.
    pub history: Vec<ConversationEntry>,
    /// A copy of the current configuration.
    pub config: EneConfig,
    /// Current session ID.
    pub session_id: SessionId,
    /// Character card name.
    pub card_name: CardName,
    /// Memory query handle (enabled only if memory is configured).
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

/// A single entry in the conversation history.
#[derive(Debug, Clone)]
pub struct ConversationEntry {
    /// Who produced this message.
    pub role: Role,
    /// The message content.
    pub content: String,
}

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

/// Thread-safe handle to the actor.
///
/// Spawns the actor on construction. When the last clone is dropped the
/// underlying `mpsc` channel closes, and the actor exits naturally.
pub struct EneHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    event_tx: broadcast::Sender<EneEvent>,
    /// `JoinHandle` for the actor task. Used by [`EneHandle::shutdown`]
    /// to await the actor's drain after sending `EneCommand::Shutdown`.
    /// Wrapped in a `tokio::sync::Mutex` because `JoinHandle` is `!Sync`
    /// and `EneHandle` must be `Clone` (and therefore `Sync`).
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
            actor_handle: Arc::clone(&self.actor_handle),
        }
    }
}

impl EneHandle {
    /// Create a new actor and return a handle to it.
    ///
    /// The actor runs as a background tokio task. When all handles are
    /// dropped the channel closes, which causes the actor to exit.
    #[must_use]
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let cmd_tx = Arc::new(cmd_tx);
        let (event_tx, _event_rx) = broadcast::channel(1024);

        let actor = EneActor::new(cmd_rx, event_tx.clone());
        let join = tokio::spawn(actor.run());

        Self {
            cmd_tx,
            event_tx,
            actor_handle: Arc::new(tokio::sync::Mutex::new(Some(join))),
        }
    }

    /// Subscribe to the event stream. Returns a receiver that will see
    /// events from this point forward.
    #[must_use]
    pub fn subscribe(&self) -> EneEventReceiver {
        EneEventReceiver(self.event_tx.subscribe())
    }

    pub(crate) fn send(&self, cmd: EneCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Send a `Run` command. Returns an error if the actor is no longer running.
    pub fn run(&self, input: impl Into<String>) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::Run {
                input: input.into(),
            })
            .map_err(|_| ActorDeadError)
    }

    /// Send a `Cancel` command. Returns an error if the actor is no longer running.
    pub fn cancel(&self) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::Cancel)
            .map_err(|_| ActorDeadError)
    }

    /// Send a `Shutdown` command and await the actor's drain.
    ///
    /// The actor's command loop returns `false` for `Shutdown`, exits,
    /// and drops its tool registry / memory store / tool RAG handles.
    /// Tool processes get killed via the registry's `Drop` impl, and
    /// the memory store's pending inserts (see `spawn_insert_log`)
    /// finish in their own `tokio::spawn`'d tasks.
    ///
    /// Returns `Ok(())` if the actor finished within the timeout, or
    /// `Err(ShutdownTimeout(Duration))` if it took longer. The actor
    /// task is detached in that case (we cannot cancel it from the
    /// other side), so the process exit that follows still aborts
    /// it; the timeout is purely to bound the wait.
    ///
    /// `Drop` already sends `Shutdown` when the last handle is dropped,
    /// so calling this method is only needed when the caller wants to
    /// observe the actor's drain (e.g. `/quit` in the CLI).
    pub async fn shutdown(&self, timeout: std::time::Duration) -> Result<(), ShutdownTimeout> {
        if self.cmd_tx.send(EneCommand::Shutdown).is_err() {
            // Actor already gone (channel closed). Drain whatever
            // JoinHandle is still around so we don't leak.
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
                // Drop the JoinHandle from the Option so a second
                // shutdown call becomes a no-op.
                guard.take();
                Ok(())
            }
            Ok(Err(join_err)) => {
                // Tokio task was cancelled or panicked. Treat as
                // completed: the actor is gone either way.
                tracing::warn!(component = "EneHandle", error = %join_err, "Actor task ended with error");
                guard.take();
                Ok(())
            }
            Err(_elapsed) => Err(ShutdownTimeout(timeout)),
        }
    }

    /// Send a permission decision for a pending destructive operation.
    /// Returns an error if the actor is no longer running.
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
    /// Returns an error if the actor is no longer running.
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

    /// Send a `Reconfigure` command and wait for the result.
    pub async fn reconfigure(&self, config: EneConfig) -> Result<(), EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::Reconfigure { config, reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)?
    }

    /// Load the configuration from default paths (assets/settings.json) and environment variables, and apply it.
    ///
    /// This is a convenience wrapper around [`ene_config::load_config`] and [`EneHandle::reconfigure`].
    /// Returns a clone of the loaded [`EneConfig`].
    pub async fn load_config(&self) -> Result<EneConfig, EneCoreError> {
        let config = ene_config::load_config()?;
        self.reconfigure(config.clone()).await?;
        Ok(config)
    }

    /// Load the configuration from specified asset and configuration paths, and apply it.
    ///
    /// This is a convenience wrapper around [`ene_config::load_config_from`] and [`EneHandle::reconfigure`].
    /// Returns a clone of the loaded [`EneConfig`].
    pub async fn load_config_from(
        &self,
        assets_dir: &std::path::Path,
        config_path: &std::path::Path,
    ) -> Result<EneConfig, EneCoreError> {
        let config = ene_config::load_config_from(assets_dir, config_path)?;
        self.reconfigure(config.clone()).await?;
        Ok(config)
    }

    /// Load a character card by its name or path.
    ///
    /// Bare names (e.g., "Alicia") are automatically resolved to their full path
    /// (e.g., `assets/characters/Alicia/character.json`).
    pub async fn load_character(&self, name: impl Into<String>) -> Result<(), EneCoreError> {
        let name = name.into();
        let path = ene_config::resolve_character_path(&name);
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::LoadCharacter { path, reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)?
    }

    /// Request a snapshot of the current actor state.
    pub async fn get_snapshot(&self) -> Result<EneStateSnapshot, EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::GetSnapshot { reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)
    }

    /// Request a manual session split. Returns the split result (summary,
    /// key facts, new session ID) when the actor has completed the split.
    ///
    /// Internally dispatches to `ene_cognition::context::execute_compression` (no
    /// `EneEvent::SessionSplit` broadcast, session ID unchanged) when cognition and compression
    /// are both enabled, or to the legacy `ene_session::execute_split` (broadcasts
    /// `EneEvent::SessionSplit`, mints a new session ID) otherwise. See
    /// [Streaming Events: the `SessionSplit` / compression gap](../../docs/core/streaming-events.md#the-sessionsplit--compression-gap).
    pub async fn manual_split(&self) -> Result<SplitResult, EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::ManualSplit { reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)?
    }

    /// List available tools from the registry.
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>, EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::ListTools { reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)
    }

    /// Call a tool directly by name with arguments.
    pub async fn call_tool(&self, name: String, arguments: String) -> Result<String, EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::CallTool {
            name,
            arguments,
            reply: tx,
        });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)?
    }

    /// Invalidate the Tool RAG index, forcing re-embedding on next query.
    pub fn invalidate_tool_index(&self) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::InvalidateToolIndex)
            .map_err(|_| ActorDeadError)
    }
}

impl Default for EneHandle {
    fn default() -> Self {
        Self::new()
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
    config: EneConfig,
    session: ConversationSession,
    registry: Arc<dyn ToolRegistry>,
    tool_rag: Option<Arc<ene_tool_host::ToolRag>>,
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<ConversationSession>>,
    pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    pending_split: Option<PendingSplitTask>,
    pending_compression: Option<PendingCompressionTask>,
    /// Background tasks spawned for direct `EneCommand::CallTool`
    /// invocations. Tracked so we can abort them cleanly on
    /// shutdown and detect the completion of an in-flight call
    /// without leaking a detached future.
    call_tool_tasks: tokio::task::JoinSet<()>,
    /// Shared with the running stream task; the first party to
    /// transition this from `false` to `true` is the one that emits
    /// [`EneEvent::Terminal`] for the current run, guaranteeing
    /// exactly one terminal event per `EneCommand::Run`.
    terminal_emitted: Arc<AtomicBool>,
}

impl EneActor {
    fn new(
        cmd_rx: mpsc::UnboundedReceiver<EneCommand>,
        event_tx: broadcast::Sender<EneEvent>,
    ) -> Self {
        LlmProviderRegistry::register(Arc::new(ene_provider::OpenAiProviderFactory));
        Self {
            cmd_rx,
            event_tx,
            config: EneConfig::default(),
            session: ConversationSession::new(),
            registry: Arc::new(CompositeToolRegistry::new(vec![])),
            tool_rag: None,
            cancel_token: CancellationToken::new(),
            stream_handle: None,
            stream_session_rx: None,
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            pending_split: None,
            pending_compression: None,
            call_tool_tasks: tokio::task::JoinSet::new(),
            terminal_emitted: Arc::new(AtomicBool::new(false)),
        }
    }

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
                            Ok(updated_session) => {
                                self.session = updated_session;
                            }
                            Err(_) => {
                                // Sender dropped without sending;
                                // keep the previous session.
                            }
                        }
                        self.stream_handle = None;
                        self.stream_session_rx = None;
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
            EneCommand::Run { input } => {
                if let Some(handle) = self.stream_handle.take() {
                    handle.abort();
                }
                // Drop any oneshot::Sender left over from the
                // previous run. The previous stream task is
                // already aborted above; this releases the
                // receivers that were awaiting decisions.
                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                // Fresh terminal guard for the new run.
                self.terminal_emitted = Arc::new(AtomicBool::new(false));
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Running,
                });
                self.start_stream(input).await;
                true
            }
            EneCommand::Cancel => {
                self.cancel_token.cancel();
                if let Some(handle) = self.stream_handle.take() {
                    handle.abort();
                }
                self.stream_session_rx = None;
                self.drain_pending().await;
                self.cancel_token = CancellationToken::new();
                // Emit Terminal(Cancelled) unless the stream task has
                // already terminated (e.g. it emitted Done/Failed
                // before this cancel landed, or the abort was clean
                // and the task already sent a terminal before
                // checking the token).
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
                    let _ = self
                        .event_tx
                        .send(EneEvent::Terminal(TerminalReason::Cancelled));
                }
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
                true
            }
            EneCommand::Shutdown => {
                // Abort any in-flight direct CallTool tasks so
                // they do not outlive the actor.
                self.call_tool_tasks.abort_all();
                self.drain_pending().await;
                false
            }
            EneCommand::Reconfigure { config, reply } => {
                let result = self.reconfigure(config).await;
                let _ = reply.send(result);
                true
            }
            EneCommand::LoadCharacter { path, reply } => {
                let result = self.load_character(&path);
                let _ = reply.send(result);
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
                let history: Vec<ConversationEntry> = self
                    .session
                    .history()
                    .iter()
                    .map(|(role, content)| ConversationEntry {
                        role: *role,
                        content: content.clone(),
                    })
                    .collect();
                let snapshot = EneStateSnapshot {
                    character_card: self.session.character_card.clone(),
                    history,
                    config: self.config.clone(),
                    session_id: self.session.memory.session_id.clone(),
                    card_name: CardName::from(self.session.card_name()),
                    memory: MemoryQueryHandle {
                        store: self.session.memory.memory_store.clone(),
                        embedder: self.session.memory.embedding_provider.clone(),
                    },
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
                        .map_err(EneCoreError::from);
                    let _ = reply.send(result);
                });
                true
            }
            EneCommand::InvalidateToolIndex => {
                self.tool_rag = None;
                true
            }
        }
    }

    async fn start_stream(&mut self, user_input: String) {
        // 1. Apply any pending split/compression result from the previous run
        self.apply_pending_split();
        self.apply_pending_compression();

        // 2. Record the triggering user message *before* taking the split
        //    snapshot so the summary includes this turn, and so the snapshot
        //    boundary points at it. apply_split_result will keep this entry
        //    and any messages added after the snapshot.
        self.session.record_user_input();
        self.session.add_user_message(&user_input);

        // 3. Check and spawn a split or compression task for this input
        self.check_and_perform_split(&user_input);

        // 5. Create provider
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
                    let _ = self
                        .event_tx
                        .send(EneEvent::Terminal(TerminalReason::Failed {
                            message: e.to_string(),
                        }));
                }
                return;
            }
        };

        // 6. Clone state for the stream task
        let config = self.config.clone();
        let session = self.session.clone();
        let embedder = self.session.memory.embedding_provider.clone();
        let registry = self.registry.clone();
        let tool_rag = self.tool_rag.clone();
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let pending_permissions = self.pending_permissions.clone();
        let pending_user_inputs = self.pending_user_inputs.clone();
        let terminal_emitted = self.terminal_emitted.clone();

        // 7. Spawn the stream task
        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let handle = tokio::spawn(async move {
            let updated_session = streaming::run_stream(streaming::StreamContext {
                config,
                session,
                user_input,
                embedder,
                registry,
                tool_rag,
                provider,
                event_tx,
                cancel_token,
                pending_permissions,
                pending_user_inputs,
                terminal_emitted,
            })
            .await;
            let _ = session_tx.send(updated_session);
        });
        self.stream_handle = Some(handle);
    }

    fn create_provider(&self) -> Result<Arc<dyn ene_provider::LlmProvider>, EneCoreError> {
        let provider_config = self.config.get_section::<ene_provider::ProviderConfig>()?;
        LlmProviderRegistry::create_provider(&provider_config.name, &self.config)
            .map(Arc::from)
            .map_err(EneCoreError::from)
    }

    // ── Split management ──

    async fn handle_manual_split(&mut self) -> Result<SplitResult, EneCoreError> {
        if self.session.history().is_empty() {
            return Err(EneCoreError::Session(EneSessionError::SplitNotNeeded));
        }
        let cognition = self
            .config
            .get_section::<ene_cognition::CognitionConfig>()
            .unwrap_or_default();
        if cognition.enabled && cognition.context.compression_enabled {
            return self.handle_manual_compression().await;
        }

        let Some(store) = &self.session.memory.memory_store else {
            return Err(EneCoreError::Session(EneSessionError::SplitNotNeeded));
        };
        let Some(embedder) = &self.session.memory.embedding_provider else {
            return Err(EneCoreError::Session(EneSessionError::SplitNotNeeded));
        };
        let provider = self.create_provider()?;

        let reason = ene_session::SplitReason::Manual;
        let result = ene_session::execute_split(
            self.session.history(),
            self.session.memory.session_id.as_str(),
            self.session.card_name(),
            &self.config.user_name,
            store,
            embedder,
            provider.as_ref(),
            reason,
        )
        .await
        .map_err(EneCoreError::Session)?;

        // Emit the split event and update the actor's session state
        let _ = self.event_tx.send(EneEvent::SessionSplit {
            summary: result.summary.clone(),
            reason: result.reason.clone(),
        });
        self.session.apply_split_result(&result);

        Ok(result)
    }

    async fn handle_manual_compression(&mut self) -> Result<SplitResult, EneCoreError> {
        let Some(store) = self.session.memory.memory_store.clone() else {
            return Err(EneCoreError::Session(EneSessionError::SplitNotNeeded));
        };
        let provider = self.create_provider()?;
        let cognition = self
            .config
            .get_section::<ene_cognition::CognitionConfig>()
            .unwrap_or_default();
        let turns = self.cognition_history_entries();
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
            config: cognition.context.clone(),
        };
        let result = execute_compression(store, provider, input).await?;
        if compression_has_usable_summary(&result) {
            self.trim_history_after_compression();
        }
        Ok(SplitResult {
            reason: ene_session::SplitReason::Manual,
            summary: result.summary,
            key_facts: vec![],
            new_session_id: self.session.memory.session_id.clone(),
            snapshot_len: self.session.history().len(),
        })
    }

    fn apply_pending_split(&mut self) {
        if let Some(result) = poll_split_result(&mut self.pending_split) {
            match result {
                Ok(split) => {
                    let _ = self.event_tx.send(EneEvent::SessionSplit {
                        summary: split.summary.clone(),
                        reason: split.reason.clone(),
                    });
                    self.session.apply_split_result(&split);
                }
                Err(e) => {
                    if !matches!(e, ene_session::EneSessionError::SplitNotNeeded) {
                        tracing::error!(component = "Session", error = %e, "Summary generation error");
                    }
                    // Split failed or wasn't needed: clear the marker so
                    // history trimming resumes.
                    self.session.clear_split_pending();
                }
            }
        }
    }

    fn check_and_perform_split(&mut self, user_input: &str) {
        let cognition = self
            .config
            .get_section::<ene_cognition::CognitionConfig>()
            .unwrap_or_default();
        if cognition.enabled && cognition.context.compression_enabled {
            self.check_and_trigger_compression();
            return;
        }

        let mem_config = match self.config.get_section::<ene_memory::MemoryConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        let session_config = match self.config.get_section::<ene_session::SessionConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };

        if !mem_config.enabled || !session_config.auto_split {
            return;
        }

        if self.pending_split.is_none() {
            let provider_config = match self.config.get_section::<ene_provider::ProviderConfig>() {
                Ok(c) => c,
                Err(_) => return,
            };
            let provider =
                match LlmProviderRegistry::create_provider(&provider_config.name, &self.config) {
                    Ok(p) => Arc::from(p),
                    Err(_) => return,
                };

            if let Some(input) = self.session.prepare_split_input(
                &self.config,
                user_input,
                &self.config.user_name.clone(),
                provider,
            ) {
                self.session.mark_split_pending();
                ene_session::spawn_split_task(&mut self.pending_split, input);
            }
        }
    }

    fn check_and_trigger_compression(&mut self) {
        let mem_config = match self.config.get_section::<ene_memory::MemoryConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        let cognition = match self.config.get_section::<ene_cognition::CognitionConfig>() {
            Ok(c) => c,
            Err(_) => return,
        };
        if !mem_config.enabled || !cognition.context.compression_enabled {
            return;
        }
        if self.pending_compression.is_some() {
            return;
        }

        let turn_count = self.session.current_turn_count();
        let history_len = self.session.history().len();
        if evaluate_compression_trigger(&cognition.context, turn_count, history_len).is_none() {
            return;
        }

        let Some(store) = self.session.memory.memory_store.clone() else {
            return;
        };
        let provider = match self.create_provider() {
            Ok(p) => p,
            Err(_) => return,
        };

        let recent_cap = cognition.context.recent_turns.saturating_mul(2).max(2);
        let history = self.session.history();
        if history.len() <= recent_cap {
            return;
        }
        let compress_count = history.len().saturating_sub(recent_cap);
        if compress_count < MIN_MESSAGES_TO_COMPRESS {
            return;
        }
        let turns: Vec<CognitionHistoryEntry> = history[..compress_count]
            .iter()
            .map(|(role, content)| CognitionHistoryEntry {
                role: match role {
                    Role::User => "user".into(),
                    Role::Assistant => "assistant".into(),
                    Role::System => "system".into(),
                },
                content: content.clone(),
            })
            .collect();
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
            config: cognition.context.clone(),
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
            .get_section::<ene_cognition::CognitionConfig>()
            .map(|c| c.context.recent_turns.saturating_mul(2).max(2))
            .unwrap_or(16);
        let history_len = self.session.history().len();
        if history_len > recent_cap {
            self.session.trim_history_keep_last(recent_cap);
        }
    }

    fn spawn_chapter_rollup_if_needed(&self) {
        let cognition = match self.config.get_section::<ene_cognition::CognitionConfig>() {
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
        let context = cognition.context.clone();
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

    fn cognition_history_entries(&self) -> Vec<CognitionHistoryEntry> {
        self.session
            .history()
            .iter()
            .map(|(role, content)| CognitionHistoryEntry {
                role: match role {
                    Role::User => "user".into(),
                    Role::Assistant => "assistant".into(),
                    Role::System => "system".into(),
                },
                content: content.clone(),
            })
            .collect()
    }

    // ── Config / Character ──

    async fn reconfigure(&mut self, config: EneConfig) -> Result<(), EneCoreError> {
        let cognition = config
            .get_section::<ene_cognition::CognitionConfig>()
            .unwrap_or_default();
        if cognition.enabled {
            ene_cognition::CognitionEngine::validate_config(&cognition)
                .map_err(EneCoreError::Cognition)?;
        }

        self.config = config;

        let embedder = init_embedding(&self.config)?;
        self.session.memory.embedding_provider = Some(embedder.clone());

        let mem_config = self.config.get_section::<ene_memory::MemoryConfig>()?;

        if mem_config.enabled {
            let store = init_memory_store(&self.config, &*embedder)
                .await
                .map_err(|e| {
                    EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
                })?;
            let cognition = self
                .config
                .get_section::<ene_cognition::CognitionConfig>()
                .unwrap_or_default();
            if cognition.enabled {
                store.set_legacy_write_mode(ene_memory::LegacyWriteMode::ReadOnly);
            }
            self.session.memory.memory_store = Some(store);
        }

        self.registry =
            build_tool_registry(&self.config, self.session.memory.memory_store.clone()).await?;

        // Build the ToolRag pipeline from config.
        self.tool_rag = init_tool_rag(&self.config, &embedder, &self.session);

        Ok(())
    }

    fn load_character(&mut self, path: &str) -> Result<(), EneCoreError> {
        self.session
            .load_card(path)
            .map_err(EneCoreError::Session)?;
        Ok(())
    }
}

// ── Factory / init helpers (moved from runtime.rs) ──

/// Builds the active composite tool registry based on workspace config.
/// Spawns per-tool DB IPC servers before starting tool processes.
async fn build_tool_registry(
    config: &EneConfig,
    memory_store: Option<Arc<ene_memory::MemoryStore>>,
) -> Result<Arc<dyn ToolRegistry>, EneCoreError> {
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
                EneCoreError::Tool(ene_tool_host::ToolHostError::ExecutionFailed {
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
        .map_err(EneCoreError::Tool)
}

fn init_embedding(
    config: &EneConfig,
) -> Result<Arc<dyn ene_provider::EmbeddingProvider>, ene_provider::EmbeddingError> {
    let provider_config = config
        .get_section::<ene_provider::ProviderConfig>()
        .map_err(|e| {
            ene_provider::EmbeddingError::Init(format!("Failed to load provider config: {e}"))
        })?;

    if provider_config.embedding.backend.as_str() == "local" {
        let local_cfg = &provider_config.embedding.local;
        let model_dir = ene_config::models_dir();
        let provider = ene_embedding::create_local_provider(
            &local_cfg.model,
            &local_cfg.quantization,
            model_dir,
        )?;
        Ok(Arc::from(provider))
    } else {
        let base_url = provider_config.resolve_base_url().map_err(|e| {
            ene_provider::EmbeddingError::Init(format!(
                "Failed to resolve base URL for cloud embedding: {e}"
            ))
        })?;
        let api_key = provider_config.resolve_api_key();
        let query_prefix = provider_config.embedding.query_prefix.clone();
        Ok(Arc::new(ene_provider::CloudEmbeddingProvider::new(
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
    embedder: &dyn ene_provider::EmbeddingProvider,
) -> Result<Arc<ene_memory::MemoryStore>, String> {
    let db_path = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default()
        .resolve_memory_db_path(&config.character);

    if let Some(parent) = db_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create memory DB directory: {e}"))?;
    }

    let dims = embedder.dimensions();
    let store = ene_memory::MemoryStore::open(&db_path, dims)
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
    embedder: &Arc<dyn ene_provider::EmbeddingProvider>,
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
    use super::*;

    /// Regression for #38: `EneHandle::shutdown` must send the
    /// shutdown command, wait for the actor task to finish, and
    /// return Ok(()). The Drop impl already does the no-wait
    /// version; the test pins the awaiting behavior that `/quit`
    /// and Ctrl-C now depend on.
    #[tokio::test]
    async fn shutdown_awaits_actor_drain() {
        let handle = EneHandle::new();
        // The actor's run() starts immediately and just loops on
        // cmd_rx.recv(). Shutdown should make it exit promptly.
        let result = handle.shutdown(std::time::Duration::from_secs(2)).await;
        assert!(result.is_ok(), "shutdown returned: {result:?}");

        // After shutdown, the actor's task is consumed from the
        // JoinHandle slot. A second shutdown is a no-op.
        let result2 = handle.shutdown(std::time::Duration::from_secs(2)).await;
        assert!(result2.is_ok(), "second shutdown returned: {result2:?}");
    }
}
