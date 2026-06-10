#[cfg(unix)]
use crate::db_server::DbIpcServer;
use crate::error::EneCoreError;
use crate::streaming::{self, PermissionDecision, UserInputResponse};
use crate::types::RequestId;
use chrono::{DateTime, Utc};
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
use std::sync::Arc;
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
    /// The conversation session has been split (timeout, topic change, or manual).
    SessionSplit {
        /// Generated summary of the conversation segment.
        summary: String,
        /// Why the split was triggered.
        reason: SplitReason,
    },
    /// The AI stream has completed normally. This is always the final event
    /// for a given run — consumers should break their event loop on this.
    Done,
    /// The AI stream terminated due to an error.
    Failed {
        /// Human-readable error description.
        message: String,
    },
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
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
            EneCoreError::EmbeddingError("Embedding provider not available".into())
        })?;
        embedder
            .embed_query(text)
            .await
            .map_err(|e| EneCoreError::EmbeddingError(format!("Embedding failed: {e}")))
    }

    /// Search conversation summaries by embedding similarity.
    pub fn search_summaries(
        &self,
        query_embedding: &[f32],
        card_name: &str,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<ene_memory::RecalledSummary>, EneCoreError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| EneCoreError::EmbeddingError("Memory store not available".into()))?;
        store
            .search_summaries(query_embedding, card_name, limit, threshold)
            .map_err(EneCoreError::Memory)
    }

    /// List recent conversation summaries for a character card.
    pub fn list_recent_summaries(
        &self,
        card_name: &str,
        limit: usize,
    ) -> Result<Vec<ene_memory::ConversationSummary>, EneCoreError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| EneCoreError::EmbeddingError("Memory store not available".into()))?;
        store
            .list_recent_summaries(card_name, limit)
            .map_err(EneCoreError::Memory)
    }

    /// List all known key facts for a character card.
    pub fn get_all_keyfacts(
        &self,
        card_name: &str,
    ) -> Result<Vec<ene_memory::KeyFact>, EneCoreError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| EneCoreError::EmbeddingError("Memory store not available".into()))?;
        store
            .get_all_keyfacts(card_name)
            .map_err(EneCoreError::Memory)
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
        tokio::spawn(actor.run());

        Self { cmd_tx, event_tx }
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
        let config = ene_config::load_config();
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
        let config = ene_config::load_config_from(assets_dir, config_path);
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
        }
    }

    async fn run(mut self) {
        loop {
            if self.stream_session_rx.is_some() {
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
                    // Regularly check stream completion while active,
                    // so sessions update promptly even without new commands.
                    () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        self.check_stream_completion();
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
    }

    fn check_stream_completion(&mut self) {
        let rx = match self.stream_session_rx.as_mut() {
            Some(rx) => rx,
            None => return,
        };

        match rx.try_recv() {
            Ok(updated_session) => {
                self.session = updated_session;
                self.stream_handle = None;
                self.stream_session_rx = None;
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
            Err(oneshot::error::TryRecvError::Closed) => {
                self.stream_handle = None;
                self.stream_session_rx = None;
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
            }
        }
    }

    async fn handle_command(&mut self, cmd: EneCommand) -> bool {
        match cmd {
            EneCommand::Run { input } => {
                if let Some(handle) = self.stream_handle.take() {
                    handle.abort();
                }
                self.cancel_token = CancellationToken::new();
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
                self.cancel_token = CancellationToken::new();
                let _ = self.event_tx.send(EneEvent::StatusChanged {
                    status: EneStatus::Idle,
                });
                true
            }
            EneCommand::Shutdown => false,
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
                tokio::spawn(async move {
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
        // 1. Apply any pending split result from the previous run
        self.apply_pending_split();

        // 2. Check and spawn a split task for this input
        self.check_and_perform_split(&user_input);

        // 3. Embed the input
        if let Err(e) = self.embed_input(&user_input).await {
            let _ = self.event_tx.send(EneEvent::Failed {
                message: e.to_string(),
            });
            return;
        }

        // 4. Record user input in session
        self.session.record_user_input();
        self.session.add_user_message(&user_input);

        // 5. Create provider
        let provider = match self.create_provider() {
            Ok(p) => p,
            Err(e) => {
                let _ = self.event_tx.send(EneEvent::Failed {
                    message: e.to_string(),
                });
                return;
            }
        };

        // 6. Clone state for the stream task
        let config = self.config.clone();
        let session = self.session.clone();
        let registry = self.registry.clone();
        let tool_rag = self.tool_rag.clone();
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let pending_permissions = self.pending_permissions.clone();
        let pending_user_inputs = self.pending_user_inputs.clone();

        // 7. Spawn the stream task
        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let handle = tokio::spawn(async move {
            let updated_session = streaming::run_stream(streaming::StreamContext {
                config,
                session,
                user_input,
                registry,
                tool_rag,
                provider,
                event_tx,
                cancel_token,
                pending_permissions,
                pending_user_inputs,
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
            .map_err(EneCoreError::Provider)
    }

    // ── Split management ──

    async fn handle_manual_split(&mut self) -> Result<SplitResult, EneCoreError> {
        if self.session.history().is_empty() {
            return Err(EneCoreError::Session(EneSessionError::SplitNotNeeded));
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
        self.session.reset_session();
        self.session.memory.session_id = result.new_session_id.clone();

        Ok(result)
    }

    fn apply_pending_split(&mut self) {
        if let Some(result) = poll_split_result(&mut self.pending_split) {
            match result {
                Ok(split) => {
                    let _ = self.event_tx.send(EneEvent::SessionSplit {
                        summary: split.summary,
                        reason: split.reason,
                    });
                    self.session.reset_session();
                    self.session.memory.session_id = split.new_session_id;
                }
                Err(e) => {
                    if !matches!(e, ene_session::EneSessionError::SplitNotNeeded) {
                        tracing::error!("[Session] Summary generation error: {}", e);
                    }
                }
            }
        }
    }

    fn check_and_perform_split(&mut self, user_input: &str) {
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
                ene_session::spawn_split_task(&mut self.pending_split, input);
            }
        }
    }

    // ── Embedding ──

    async fn embed_input(&mut self, input: &str) -> Result<Vec<f32>, EneCoreError> {
        let embedder = self
            .session
            .memory
            .embedding_provider
            .clone()
            .ok_or_else(|| {
                EneCoreError::EmbeddingError("No embedding provider initialized".to_string())
            })?;

        let embedding = embedder
            .embed_query(input)
            .await
            .map_err(|e| EneCoreError::EmbeddingError(format!("Failed to embed: {e}")))?;

        self.session.set_pending_embedding(embedding.clone());
        self.session.set_last_input_embedding(embedding.clone());
        Ok(embedding)
    }

    // ── Config / Character ──

    async fn reconfigure(&mut self, config: EneConfig) -> Result<(), EneCoreError> {
        self.config = config;

        let embedder = init_embedding(&self.config).map_err(EneCoreError::EmbeddingError)?;
        self.session.memory.embedding_provider = Some(embedder.clone());

        let mem_config = self.config.get_section::<ene_memory::MemoryConfig>()?;

        if mem_config.enabled {
            let store = init_memory_store(&self.config, &*embedder).map_err(|e| {
                EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
            })?;
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
    if memory_store.is_some() {
        #[cfg(unix)]
        let tool_config = config
            .get_section::<ene_tool_host::ToolConfig>()
            .unwrap_or_default();

        #[cfg(unix)]
        let db_path = config
            .get_section::<ene_memory::MemoryConfig>()
            .unwrap_or_default()
            .resolve_memory_db_path(&config.character);

        #[cfg(unix)]
        {
            let socket_dir = ene_config::paths::tool_socket_dir();
            std::fs::create_dir_all(&socket_dir).map_err(|e| {
                EneCoreError::Tool(ene_tool_proto::ToolError::ExecutionFailed {
                    message: format!("Failed to create socket dir: {e}"),
                })
            })?;

            for (name, entry) in &tool_config.list {
                if !entry.enable {
                    continue;
                }

                let tool_name = name.clone();
                let prefix = format!("{name}_");
                let socket_path = socket_dir.join(format!("ene-db-{name}.sock"));

                let server =
                    DbIpcServer::new(db_path.clone(), socket_path, tool_name.clone(), prefix);

                tokio::spawn(async move {
                    if let Err(e) = server.run().await {
                        tracing::error!(tool = %tool_name, error = %e, "DB IPC server error");
                    }
                });
            }
        }
    }

    ToolHostManager::start_full(config)
        .await
        .map_err(EneCoreError::Tool)
}

fn init_embedding(config: &EneConfig) -> Result<Arc<dyn ene_provider::EmbeddingProvider>, String> {
    let provider_config = config
        .get_section::<ene_provider::ProviderConfig>()
        .map_err(|e| format!("Failed to load provider config: {e}"))?;

    if provider_config.embedding.backend.as_str() == "local" {
        let local_cfg = &provider_config.embedding.local;
        let model_dir = ene_config::models_dir();
        let provider = ene_embedding::create_local_provider(
            &local_cfg.model,
            &local_cfg.quantization,
            model_dir,
        )
        .map_err(|e| format!("Failed to create local embedding provider: {e}"))?;
        Ok(Arc::from(provider))
    } else {
        let base_url = provider_config
            .resolve_base_url()
            .map_err(|e| format!("Failed to resolve base URL for cloud embedding: {e}"))?;
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

fn init_memory_store(
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
        .map_err(|e| format!("Failed to open memory store: {e}"))?;

    Ok(Arc::new(store))
}

/// Builds the `ToolRag` pipeline from the current config, embedder, and session state.
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
    let opts = ene_tool_host::ToolRagOptions::from(rag_config);
    Some(Arc::new(ene_tool_host::ToolRag::new(
        embedder.clone(),
        store,
        opts,
    )))
}
