use crate::error::EneCoreError;
use crate::stream::{self, PermissionDecision};
use chrono::{DateTime, Utc};
use ene_config::EneConfig;
use ene_provider::LlmProviderRegistry;
use ene_session::PendingSplitTask;
use ene_session::{ConversationSession, SessionError, SplitResult, poll_split_result};
use ene_tool_host::{CompositeToolRegistry, ToolHostManager, ToolRegistry};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// Commands sent to the actor from consumers (UI/CLI).
///
/// Fire-and-forget variants are sent via [`EneHandle::send`].
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
        request_id: String,
        /// The user's decision.
        decision: PermissionDecision,
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
}

/// Events emitted from the actor to all consumers via broadcast channel.
///
/// Consumers (CLI, Bevy systems, logging) receive these through
/// [`EneHandle::try_recv`] or [`EneHandle::recv`].
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
        request_id: String,
        /// The category of operation (e.g. "write", "delete").
        action: String,
        /// The target resource path.
        target: String,
        /// Human-readable description of what will be done.
        description: String,
    },
    /// Progress update for a long-running background task.
    TaskProgress {
        /// Unique task identifier.
        task_id: String,
        /// Current step number.
        step: usize,
        /// Total number of steps.
        total_steps: usize,
        /// Description of the current step.
        description: String,
    },
    /// The conversation session has been split (timeout, topic change, or manual).
    SessionSplit {
        /// Generated summary of the conversation segment.
        summary: String,
        /// Why the split was triggered.
        reason: String,
    },
    /// The AI stream has completed normally.
    Finished,
    /// A non-fatal error occurred during streaming.
    Error {
        /// Human-readable error description.
        message: String,
    },
    /// The actor's status changed.
    StatusChanged {
        /// New status value.
        status: EneStatus,
    },
}

/// A snapshot of the current actor state for read-only queries.
#[derive(Clone)]
pub struct EneStateSnapshot {
    /// The loaded character card, if any.
    pub character_card: Option<ene_session::CharacterCardV3>,
    /// Conversation history.
    pub history: Vec<(ene_session::Role, String)>,
    /// A copy of the current configuration.
    pub config: EneConfig,
    /// Current session ID.
    pub session_id: String,
    /// Character card name.
    pub card_name: String,
    /// Memory store, if enabled.
    pub memory_store: Option<Arc<ene_memory::MemoryStore>>,
    /// Embedding provider, if initialized.
    pub embedding_provider: Option<Arc<dyn ene_provider::EmbeddingProvider>>,
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

/// Thread-safe handle to the actor.
///
/// Spawns the actor on construction. When the last clone is dropped the
/// underlying `mpsc` channel closes, and the actor exits naturally.
pub struct EneHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    event_tx: broadcast::Sender<EneEvent>,
    event_rx: broadcast::Receiver<EneEvent>,
}

impl Clone for EneHandle {
    fn clone(&self) -> Self {
        Self {
            cmd_tx: Arc::clone(&self.cmd_tx),
            event_tx: self.event_tx.clone(),
            event_rx: self.event_tx.subscribe(),
        }
    }
}

impl EneHandle {
    /// Create a new actor and return a handle to it.
    ///
    /// The actor runs as a background tokio task. When all handles are
    /// dropped the channel closes, which causes the actor to exit.
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let cmd_tx = Arc::new(cmd_tx);
        let (event_tx, event_rx) = broadcast::channel(1024);

        let actor = EneActor::new(cmd_rx, event_tx.clone());
        tokio::spawn(actor.run());

        Self {
            cmd_tx,
            event_tx,
            event_rx,
        }
    }

    /// Obtain a fresh broadcast receiver that sees events from this point
    /// forward. Prefer this over `clone()` when you only need to consume
    /// events and want to avoid the lifetime implications of cloning the
    /// entire handle.
    pub fn subscribe(&self) -> broadcast::Receiver<EneEvent> {
        self.event_tx.subscribe()
    }

    /// Send a command (fire-and-forget).
    pub fn send(&self, cmd: EneCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Convenience: send a `Run` command.
    pub fn run(&self, input: impl Into<String>) {
        self.send(EneCommand::Run {
            input: input.into(),
        });
    }

    /// Convenience: send a `Cancel` command.
    pub fn cancel(&self) {
        self.send(EneCommand::Cancel);
    }

    /// Non-blocking poll of the event stream (for Bevy ECS systems).
    pub fn try_recv(&mut self) -> Result<EneEvent, broadcast::error::TryRecvError> {
        self.event_rx.try_recv()
    }

    /// Async receive (for tokio tasks).
    pub async fn recv(&mut self) -> Result<EneEvent, broadcast::error::RecvError> {
        self.event_rx.recv().await
    }

    /// Send a `Reconfigure` command and wait for the result.
    pub async fn reconfigure(&self, config: EneConfig) -> Result<(), EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::Reconfigure { config, reply: tx });
        rx.await.map_err(|_| EneCoreError::ChannelClosed)?
    }

    /// Send a `LoadCharacter` command and wait for the result.
    pub async fn load_character(&self, path: impl Into<String>) -> Result<(), EneCoreError> {
        let (tx, rx) = oneshot::channel();
        self.send(EneCommand::LoadCharacter {
            path: path.into(),
            reply: tx,
        });
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
}

impl Default for EneHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EneHandle {
    fn drop(&mut self) {
        // Only the last handle sends Shutdown so the actor can clean up.
        // When all Arc<Sender> clones are dropped the mpsc channel closes
        // automatically, but an explicit Shutdown is faster.
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
    cancel_token: CancellationToken,
    stream_handle: Option<tokio::task::JoinHandle<()>>,
    stream_session_rx: Option<oneshot::Receiver<ConversationSession>>,
    pending_permissions: Arc<Mutex<HashMap<String, oneshot::Sender<PermissionDecision>>>>,
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
            cancel_token: CancellationToken::new(),
            stream_handle: None,
            stream_session_rx: None,
            pending_permissions: Arc::new(Mutex::new(HashMap::new())),
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
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
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
            EneCommand::ManualSplit { reply } => {
                let result = self.handle_manual_split().await;
                let _ = reply.send(result);
                true
            }
            EneCommand::GetSnapshot { reply } => {
                let snapshot = EneStateSnapshot {
                    character_card: self.session.character_card.clone(),
                    history: self.session.history.conversation_history.clone(),
                    config: self.config.clone(),
                    session_id: self.session.memory.session_id.clone(),
                    card_name: self.session.card_name().to_string(),
                    memory_store: self.session.memory.memory_store.clone(),
                    embedding_provider: self.session.memory.embedding_provider.clone(),
                    current_turn_count: self.session.state.current_turn_count as u32,
                    session_started_at: self.session.memory.session_started_at,
                };
                let _ = reply.send(snapshot);
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
            let _ = self.event_tx.send(EneEvent::Error {
                message: e.to_string(),
            });
            let _ = self.event_tx.send(EneEvent::Finished);
            return;
        }

        // 4. Record user input in session
        self.session.record_user_input();
        self.session.add_user_message(&user_input);

        // 5. Create provider
        let provider = match self.create_provider() {
            Ok(p) => p,
            Err(e) => {
                let _ = self.event_tx.send(EneEvent::Error {
                    message: e.to_string(),
                });
                let _ = self.event_tx.send(EneEvent::Finished);
                return;
            }
        };

        // 6. Clone state for the stream task
        let config = self.config.clone();
        let session = self.session.clone();
        let registry = self.registry.clone();
        let event_tx = self.event_tx.clone();
        let cancel_token = self.cancel_token.clone();
        let pending_permissions = self.pending_permissions.clone();

        // 7. Spawn the stream task
        let (session_tx, session_rx) = oneshot::channel();
        self.stream_session_rx = Some(session_rx);

        let handle = tokio::spawn(async move {
            let updated_session = stream::run_stream(
                &config,
                &session,
                &user_input,
                registry,
                provider,
                event_tx,
                cancel_token,
                pending_permissions,
            )
            .await;
            let _ = session_tx.send(updated_session);
        });
        self.stream_handle = Some(handle);
    }

    fn create_provider(&self) -> Result<Arc<dyn ene_provider::LlmProvider>, EneCoreError> {
        let provider_config = self
            .config
            .get_section::<ene_provider::ProviderConfig>()
            .map_err(|e| EneCoreError::ConfigError(e.to_string()))?;
        LlmProviderRegistry::create_provider(&provider_config.provider_name, &self.config)
            .map(Arc::from)
            .map_err(|e| EneCoreError::ConfigError(e))
    }

    // ── Split management ──

    async fn handle_manual_split(&mut self) -> Result<SplitResult, EneCoreError> {
        if self.session.history.conversation_history.is_empty() {
            return Err(EneCoreError::Session(SessionError::SplitNotNeeded));
        }
        let Some(store) = &self.session.memory.memory_store else {
            return Err(EneCoreError::Session(SessionError::SplitNotNeeded));
        };
        let Some(embedder) = &self.session.memory.embedding_provider else {
            return Err(EneCoreError::Session(SessionError::SplitNotNeeded));
        };
        let provider = self.create_provider()?;

        let reason = ene_session::SplitReason::Manual;
        let result = ene_session::execute_split(
            &self.session.history.conversation_history,
            &self.session.memory.session_id,
            self.session.card_name(),
            &self.config.user_name,
            store,
            embedder,
            provider.as_ref(),
            reason,
        )
        .await
        .map_err(|e| EneCoreError::Session(e))?;

        // Emit the split event and update the actor's session state
        let _ = self.event_tx.send(EneEvent::SessionSplit {
            summary: result.summary.clone(),
            reason: result.reason.to_string(),
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
                        reason: split.reason.to_string(),
                    });
                    self.session.reset_session();
                    self.session.memory.session_id = split.new_session_id;
                }
                Err(e) => {
                    if !matches!(e, ene_session::SessionError::SplitNotNeeded) {
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

        if !mem_config.enabled || !session_config.auto_session_split {
            return;
        }

        if self.pending_split.is_none() {
            let provider_config = match self.config.get_section::<ene_provider::ProviderConfig>() {
                Ok(c) => c,
                Err(_) => return,
            };
            let provider = match LlmProviderRegistry::create_provider(
                &provider_config.provider_name,
                &self.config,
            ) {
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
            .map_err(|e| EneCoreError::EmbeddingError(format!("Failed to embed: {}", e)))?;

        self.session.set_pending_embedding(embedding.clone());
        self.session.set_last_input_embedding(embedding.clone());
        Ok(embedding)
    }

    // ── Config / Character ──

    async fn reconfigure(&mut self, config: EneConfig) -> Result<(), EneCoreError> {
        self.config = config;

        let embedder = init_embedding(&self.config).map_err(EneCoreError::EmbeddingError)?;
        self.session.memory.embedding_provider = Some(embedder.clone());

        let mem_config = self
            .config
            .get_section::<ene_memory::MemoryConfig>()
            .map_err(|e| {
                EneCoreError::ConfigError(format!("Failed to load memory config: {}", e))
            })?;

        if mem_config.enabled {
            let store = init_memory_store(&self.config, &*embedder).map_err(|e| {
                EneCoreError::Memory(ene_memory::MemoryError::MemoryStoreConnectionError(e))
            })?;
            self.session.memory.memory_store = Some(store);
        }

        self.registry = build_tool_registry(&self.config).await?;

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
async fn build_tool_registry(config: &EneConfig) -> Result<Arc<dyn ToolRegistry>, EneCoreError> {
    ToolHostManager::start_full(config).await.map_err(|e| {
        EneCoreError::ConfigError(format!("Fatal: Failed to start ToolHostManager: {}", e))
    })
}

fn init_embedding(config: &EneConfig) -> Result<Arc<dyn ene_provider::EmbeddingProvider>, String> {
    let provider_config = config
        .get_section::<ene_provider::ProviderConfig>()
        .map_err(|e| format!("Failed to load provider config: {}", e))?;

    match provider_config.embedding_backend.as_str() {
        "local" => {
            let local_cfg = ene_provider::ProviderConfig::local_embedding(config);
            let model_dir = ene_config::models_dir();
            let provider = ene_embedding::create_local_provider(
                &local_cfg.model,
                &local_cfg.quantization,
                model_dir,
            )
            .map_err(|e| format!("Failed to create local embedding provider: {}", e))?;
            Ok(Arc::from(provider))
        }
        _ => {
            let base_url = provider_config
                .resolve_base_url()
                .map_err(|e| format!("Failed to resolve base URL for cloud embedding: {}", e))?;
            let api_key = provider_config.resolve_api_key();
            let llm: Arc<dyn ene_provider::LlmProvider> =
                Arc::new(ene_provider::OpenAiProvider::new(
                    &base_url,
                    &api_key,
                    &provider_config.model,
                    &provider_config.cloud_embedding_model,
                    provider_config.cloud_embedding_dimensions,
                ));
            Ok(Arc::new(ene_provider::CloudEmbeddingProvider::new(llm)))
        }
    }
}

fn init_memory_store(
    config: &EneConfig,
    embedder: &dyn ene_provider::EmbeddingProvider,
) -> Result<Arc<ene_memory::MemoryStore>, String> {
    let db_path = config
        .get_section::<ene_memory::MemoryConfig>()
        .unwrap_or_default()
        .resolve_memory_db_path();

    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create memory DB directory: {}", e))?;
        }
    }

    let dims = embedder.dimensions();
    let store = ene_memory::MemoryStore::open(&db_path, dims)
        .map_err(|e| format!("Failed to open memory store: {}", e))?;

    Ok(Arc::new(store))
}
