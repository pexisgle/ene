//! Actor-based runtime with message-passing architecture.
//!
//! [`EneHandle`] is the thread-safe facade constructed by [`EneHandle::open`].
//! It owns the command channel into the single-threaded [`actor::TurnActor`]
//! (turn execution, permissions, undo, tool calls, character/config control)
//! and delegates read-only session/candidate queries and screen-image vision
//! summarization to dedicated handles that bypass the actor mailbox entirely
//! (#271) — see [`EneHandle::sessions`], [`EneHandle::candidates`], and
//! [`EneHandle::vision`].
//!
//! ## Module layout (#271)
//!
//! - [`command`] — [`EneCommand`] (now solely turn-execution / control-plane
//!   commands; the session/candidate/vision-payload variants are gone).
//! - [`event`] — [`EneEvent`] (chat bus), [`AudioChunk`] (audio channel),
//!   [`LifecycleEvent`] (lifecycle bus), [`TerminalReason`], [`EneStatus`],
//!   [`EneEventReceiver`], [`AudioStreamReceiver`], [`LifecycleReceiver`],
//!   [`EneStateSnapshot`]. See the module doc there for the three-channel
//!   event bus design (#272).
//! - [`actor`] — [`actor::TurnActor`] (formerly `EneActor`) and its
//!   supporting free functions.
//! - [`crate::query::sessions`] / [`crate::query::candidates`] — read-only
//!   session and pending-candidate handles.
//! - [`crate::vision`] — screen-image vision summarization handle.

mod actor;
mod command;
mod event;

/// Re-exported so `ene_runtime::handle::PendingCandidateSummary` (the
/// pre-#271 path some embedders imported) keeps resolving after the type
/// moved to [`crate::query::candidates`].
pub use crate::query::candidates::PendingCandidateSummary;
pub use command::{DeferredToolTask, EneCommand, FeatureSettingsUpdate};
pub use event::{
    AudioChunk, AudioStreamReceiver, EneEvent, EneEventReceiver, EneStateSnapshot, EneStatus,
    LifecycleEvent, LifecycleReceiver, TerminalReason,
};

use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::error::EneRuntimeError;
use crate::streaming::{PermissionDecision, UserInputResponse};
use crate::types::{CancelError, RequestId, RunError, TurnId};
use ene_ai::LlmProviderRegistry;
use ene_config::CharacterCardV3;
use ene_config::EneConfig;
use ene_mind::ConversationSession;
use ene_plugin_host::PluginHealthEvent;
use ene_tool_rag::ToolRagConfig;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// Bounded capacity for the dedicated audio (PCM) channel (#272).
///
/// Chosen to buffer several seconds of TTS chunks (each chunk is roughly one
/// sentence of synthesized speech) ahead of a real-time playback consumer
/// without letting an unconsumed stream grow without bound. See
/// `send_audio_chunk` in `streaming_cognitive.rs` for the back-pressure
/// policy applied when this fills up.
const AUDIO_CHANNEL_CAPACITY: usize = 64;

/// Small broadcast capacity for the lifecycle bus (#272).
///
/// Lifecycle notifications (`StatusChanged`, `PendingCandidateAvailable`,
/// `ToolBackgroundCompleted`) are low-frequency and turn-independent, so a
/// much smaller buffer than the chat bus's 1024 is sufficient.
const LIFECYCLE_CHANNEL_CAPACITY: usize = 64;

/// Error returned when a command is sent to an actor that is no longer running.
#[derive(Debug, thiserror::Error)]
#[error("Actor is no longer running")]
pub struct ActorDeadError;

/// Error returned by [`EneHandle::shutdown`] when the actor's drain
/// takes longer than the supplied timeout.
#[derive(Debug, thiserror::Error)]
#[error("Actor did not shut down within {0:?}")]
pub struct ShutdownTimeout(pub std::time::Duration);

/// Shared single-flight turn gate between handle and actor.
///
/// Uses a single `parking_lot::Mutex<Option<TurnId>>` for both the
/// busy flag and active turn ID, eliminating the split atomic/mutex
/// design and the ordering window it created.
struct TurnGate {
    active: parking_lot::Mutex<Option<TurnId>>,
}

impl TurnGate {
    fn new() -> Self {
        Self {
            active: parking_lot::Mutex::new(None),
        }
    }

    fn try_begin(&self, turn: &TurnId) -> bool {
        let mut guard = self.active.lock();
        if guard.is_some() {
            return false;
        }
        *guard = Some(turn.clone());
        true
    }

    fn end(&self) {
        *self.active.lock() = None;
    }

    fn matches(&self, turn: &TurnId) -> bool {
        self.active
            .lock()
            .as_ref()
            .is_some_and(|active| active.as_str() == turn.as_str())
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
    /// Lifecycle bus sender (#272): `StatusChanged` / `PendingCandidateAvailable`
    /// / `ToolBackgroundCompleted`. Separate broadcast channel from `event_tx`
    /// so lifecycle traffic never shares a buffer with chat events.
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    /// Audio channel sender (#272): bounded `mpsc`, single-consumer. The
    /// paired receiver lives in `audio_rx` until [`EneHandle::take_audio_stream`]
    /// transfers it out.
    audio_tx: mpsc::Sender<AudioChunk>,
    /// Holds the audio channel's receiver until claimed by
    /// [`EneHandle::take_audio_stream`]. Shared across `EneHandle` clones
    /// (not per-clone) so ownership transfer is global, not per-clone.
    /// `parking_lot::Mutex` (sync, non-fallible lock) rather than
    /// `tokio::sync::Mutex` because `take_audio_stream` is a plain
    /// synchronous method, not `async fn`.
    audio_rx: Arc<parking_lot::Mutex<Option<mpsc::Receiver<AudioChunk>>>>,
    diag_tx: broadcast::Sender<crate::diagnostics::DiagnosticEvent>,
    diagnostics: crate::diagnostics::EneDiagnostics,
    turn_gate: Arc<TurnGate>,
    /// `JoinHandle` for the actor task. Used by [`EneHandle::shutdown`]
    /// to await the actor's drain after sending `EneCommand::Shutdown`.
    actor_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Plugin host manager (process supervision for v3 plugin binaries).
    ///
    /// Shared across handle clones; the last clone to drop triggers a
    /// graceful plugin shutdown (#247). `None` when plugin startup failed
    /// or no plugins were discovered.
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    /// Join handle for the plugin health → diagnostics bridge task (#238).
    ///
    /// Shared across handle clones. Aborted during plugin host shutdown so
    /// the bridge does not outlive the host whose events it forwards.
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Read-only session query handle, bypasses the actor mailbox (#271).
    sessions: crate::query::sessions::SessionQueryHandle,
    /// Pending memory-candidate approval handle, bypasses the actor mailbox (#271).
    candidates: crate::query::candidates::MemoryCandidateHandle,
    /// Screen-image vision summarization handle, bypasses the actor mailbox (#271).
    vision: crate::vision::VisionHandle,
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
            lifecycle_tx: self.lifecycle_tx.clone(),
            audio_tx: self.audio_tx.clone(),
            audio_rx: Arc::clone(&self.audio_rx),
            diag_tx: self.diag_tx.clone(),
            diagnostics: crate::diagnostics::EneDiagnostics {
                cmd_tx: Arc::clone(&self.cmd_tx),
                diag_tx: self.diag_tx.clone(),
                memory: self.diagnostics.memory.clone(),
                health_monitor: self.diagnostics.health_monitor.clone(),
            },
            turn_gate: Arc::clone(&self.turn_gate),
            actor_handle: Arc::clone(&self.actor_handle),
            plugin_host: Arc::clone(&self.plugin_host),
            health_bridge_handle: Arc::clone(&self.health_bridge_handle),
            sessions: self.sessions.clone(),
            candidates: self.candidates.clone(),
            vision: self.vision.clone(),
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
        let (lifecycle_tx, _lifecycle_rx) = broadcast::channel(LIFECYCLE_CHANNEL_CAPACITY);
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_CHANNEL_CAPACITY);
        let (diag_tx, diag_rx) = broadcast::channel(256);

        LlmProviderRegistry::register(Arc::new(ene_ai::OpenAiProviderFactory));

        // Register the local whisper/kokoro/silero factories with
        // `ene_ai::AudioProviderRegistry` before anything below (this
        // function's own TTS provider resolution, or a later
        // caller-initiated STT/VAD lookup such as the desktop app's mic
        // toggle) can look one up by name. Previously each factory
        // registered itself via a `#[ctor::ctor]` that ran before `main`
        // and before `tracing` was initialized; doing it here, after
        // `tracing` is up, makes registration observable.
        ene_voice::register_providers();

        let mind = config.get_section::<ene_mind::MindConfig>()?;
        ene_mind::CognitionEngine::validate_config(&mind).map_err(EneRuntimeError::from)?;

        let mem_config = config.get_section::<ene_store::StoreConfig>()?;
        let plugin_config = config.get_section::<ene_plugin_host::PluginConfig>()?;
        let rag_config = config.get_section::<ToolRagConfig>()?;
        let needs_embedder = mem_config.enabled || (plugin_config.enabled && rag_config.enabled);

        // Prefetch configured GGUF weights in parallel before backends load them.
        {
            let ai_config = config.get_section::<ene_ai::AiConfig>()?;
            let needs_decision = mind.proactive.enabled;
            if (needs_embedder || needs_decision)
                && let Err(e) = ene_ai_local::prefetch_configured_gguf(
                    &ai_config,
                    needs_embedder,
                    needs_decision,
                )
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
            Some(actor::init_embedding(&config)?)
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
            let store = actor::init_memory_store(&config, emb.as_ref())
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

        // Generate DB tokens and spawn DB IPC servers for tool plugins.
        let db_tokens = actor::spawn_db_ipc_servers(&config, memory_store.as_ref())?;

        // Start the plugin host (discovers and launches v3 plugin binaries).
        // Non-fatal: on failure we log and continue with no plugin-provided
        // providers/tools, mirroring the tool host's empty-set fallback (#247).
        let mut plugin_host =
            match ene_plugin_host::PluginHostManager::start(&config, db_tokens).await {
                Ok(host) => {
                    for (kind, factory) in host.llm_factories() {
                        tracing::info!(
                            component = "Bootstrap",
                            kind = %kind,
                            "Registered plugin-provided LLM provider factory"
                        );
                        LlmProviderRegistry::register(Arc::clone(factory));
                    }
                    tracing::info!(
                        component = "Bootstrap",
                        tool_registries = host.tool_registries().len(),
                        llm_factories = host.llm_factories().len(),
                        "Plugin host started"
                    );
                    Some(host)
                }
                Err(e) => {
                    tracing::warn!(
                        component = "Bootstrap",
                        error = %e,
                        "Plugin host failed to start; continuing without plugins"
                    );
                    None
                }
            };

        // Bridge plugin health events into the diagnostics channel (#238).
        // The task's `JoinHandle` is retained so it can be aborted during
        // plugin host shutdown rather than leaking past the host's lifetime.
        let mut health_bridge_handle: Option<tokio::task::JoinHandle<()>> = None;
        if let Some(host) = plugin_host.as_mut()
            && let Some(mut health_rx) = host.take_health_receiver()
        {
            let diag_tx = diag_tx.clone();
            health_bridge_handle = Some(tokio::spawn(async move {
                while let Some(event) = health_rx.recv().await {
                    emit_diag(&diag_tx, plugin_health_event_to_diag(event));
                }
            }));
        }

        let registry = actor::build_tool_registry(plugin_host.as_ref())?;
        let tool_rag = match embedder.as_ref() {
            Some(emb) => actor::init_tool_rag(&config, emb, &session)?,
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
        let warmup_hash = actor::warmup_character_memories_ready(&config, &session).await;

        if let Some(hash) = warmup_hash {
            session.memory.ccv3_memory_hash = Some(hash);
        }

        let mind_memory = mind.memory.clone();
        let memory = crate::diagnostics::MemoryQueryHandle::new(
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

        // Resolve TTS provider from config (None when provider is "none").
        let tts_provider = {
            let ai_config = config.get_section::<ene_ai::AiConfig>()?;

            // Prefetch the configured local TTS model files before
            // constructing the provider below, so `LocalTtsProvider::open`
            // (ene-voice) never needs to perform network I/O itself — it
            // only fails fast if a file is still missing. Mirrors the GGUF
            // prefetch above. Non-fatal: on failure we log and let provider
            // construction report a clear error.
            if let Err(e) = ene_voice::prefetch_if_configured(&ai_config).await {
                tracing::warn!(
                    component = "Bootstrap",
                    error = %e,
                    "Local TTS model prefetch failed; will report a clear error on provider construction"
                );
            }

            if let Some(resolved) = ai_config.resolve_tts() {
                match ene_ai::AudioProviderRegistry::create_tts_provider(
                    &resolved.provider,
                    &config,
                ) {
                    Ok(provider) => {
                        tracing::info!(
                            component = "Bootstrap",
                            provider = %provider.name(),
                            "TTS provider initialized"
                        );
                        Some(Arc::from(provider))
                    }
                    Err(e) => {
                        tracing::warn!(
                            component = "Bootstrap",
                            error = %e,
                            "Failed to initialize TTS provider; audio synthesis disabled"
                        );
                        None
                    }
                }
            } else {
                tracing::debug!(
                    component = "Bootstrap",
                    "TTS disabled by configuration (provider = \"none\")"
                );
                None
            }
        };

        let diagnostics = crate::diagnostics::EneDiagnostics {
            cmd_tx: Arc::clone(&cmd_tx),
            diag_tx: diag_tx.clone(),
            memory,
            health_monitor: health_monitor.clone(),
        };

        let turn_gate = Arc::new(TurnGate::new());

        let plugin_tool_registries = plugin_host
            .as_ref()
            .map_or_else(Vec::new, |h| h.tool_registries().to_vec());

        // Share the plugin host and its health-bridge task between the handle
        // (for shutdown) and the actor (for Features-update reconfiguration).
        let plugin_host = Arc::new(tokio::sync::Mutex::new(plugin_host));
        let health_bridge_handle = Arc::new(tokio::sync::Mutex::new(health_bridge_handle));

        // Current character-card name, shared with `MemoryCandidateHandle` so
        // pending-candidate queries never need a mailbox round-trip (#271).
        let card_name = Arc::new(parking_lot::Mutex::new(
            card.data.get_character_name().to_string(),
        ));

        let sessions = crate::query::sessions::SessionQueryHandle::new(memory_store.clone());
        let candidates = crate::query::candidates::MemoryCandidateHandle::new(
            memory_store.clone(),
            Arc::clone(&card_name),
        );
        let vision = crate::vision::VisionHandle::new(Arc::clone(&cmd_tx));

        let actor = actor::TurnActor::new(
            cmd_rx,
            (*cmd_tx).clone(),
            event_tx.clone(),
            lifecycle_tx.clone(),
            audio_tx.clone(),
            diag_tx.clone(),
            diag_rx,
            Arc::clone(&turn_gate),
            config,
            session,
            registry,
            tool_rag,
            health_monitor,
            tts_provider,
            plugin_tool_registries,
            Arc::clone(&plugin_host),
            Arc::clone(&health_bridge_handle),
            card_name,
        );
        let join = tokio::spawn(actor.run());

        Ok(Self {
            cmd_tx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            audio_rx: Arc::new(parking_lot::Mutex::new(Some(audio_rx))),
            diag_tx,
            diagnostics,
            turn_gate,
            actor_handle: Arc::new(tokio::sync::Mutex::new(Some(join))),
            plugin_host,
            health_bridge_handle,
            sessions,
            candidates,
            vision,
        })
    }

    /// Subscribe to the chat event stream.
    pub fn subscribe(&self) -> EneEventReceiver {
        EneEventReceiver {
            inner: self.event_tx.subscribe(),
            diag_tx: self.diag_tx.clone(),
        }
    }

    /// Subscribe to the lifecycle event stream (#272): `StatusChanged`,
    /// `PendingCandidateAvailable`, `ToolBackgroundCompleted`.
    ///
    /// Separate from [`EneHandle::subscribe`] (the chat bus) — lifecycle
    /// notifications are turn-independent and low-frequency. Multiple
    /// subscribers are allowed, same as the chat bus.
    pub fn subscribe_lifecycle(&self) -> LifecycleReceiver {
        LifecycleReceiver {
            inner: self.lifecycle_tx.subscribe(),
            diag_tx: self.diag_tx.clone(),
        }
    }

    /// Take ownership of the audio (PCM) stream (#272).
    ///
    /// Returns `Some` exactly once across every clone of this `EneHandle` —
    /// the audio channel is single-consumer by construction, modeling the
    /// current single playback-sink path. Every call after the first (from
    /// this handle or any of its clones) returns `None`.
    pub fn take_audio_stream(&self) -> Option<AudioStreamReceiver> {
        self.audio_rx
            .lock()
            .take()
            .map(|inner| AudioStreamReceiver { inner })
    }

    /// Concrete diagnostics facade (pipeline detail, memory, tools).
    pub const fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics {
        &self.diagnostics
    }

    /// Read-only session query handle (list / export / import / search /
    /// archive), bypassing the turn-execution actor mailbox entirely (#271).
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn sessions(&self) -> crate::query::sessions::SessionQueryHandle {
        self.sessions.clone()
    }

    /// Pending memory-candidate approval handle (list / approve / reject),
    /// bypassing the turn-execution actor mailbox entirely (#271).
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn candidates(&self) -> crate::query::candidates::MemoryCandidateHandle {
        self.candidates.clone()
    }

    /// Screen-image vision summarization handle, bypassing the
    /// turn-execution actor mailbox entirely (#271).
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn vision(&self) -> crate::vision::VisionHandle {
        self.vision.clone()
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

    /// Send a `Shutdown` command and await the actor's drain, then shut down
    /// the plugin host (if still alive).
    pub async fn shutdown(&self, timeout: std::time::Duration) -> Result<(), ShutdownTimeout> {
        if self.cmd_tx.send(EneCommand::Shutdown).is_err() {
            let mut guard = self.actor_handle.lock().await;
            if let Some(join) = guard.take() {
                drop(join.await);
            }
            drop(guard);
            self.shutdown_plugin_host().await;
            return Ok(());
        }

        let mut guard = self.actor_handle.lock().await;
        let Some(join) = guard.as_mut() else {
            drop(guard);
            self.shutdown_plugin_host().await;
            return Ok(());
        };
        match tokio::time::timeout(timeout, &mut *join).await {
            Ok(Ok(())) => {
                guard.take();
                drop(guard);
                self.shutdown_plugin_host().await;
                Ok(())
            }
            Ok(Err(join_err)) => {
                tracing::warn!(component = "EneHandle", error = %join_err, "Actor task ended with error");
                guard.take();
                drop(guard);
                self.shutdown_plugin_host().await;
                Ok(())
            }
            Err(_elapsed) => Err(ShutdownTimeout(timeout)),
        }
    }

    /// Gracefully shuts down the plugin host if it is still alive (#247).
    ///
    /// Called from [`EneHandle::shutdown`] after the actor drains and from
    /// [`Drop`] when the last handle clone is released. Only the first caller
    /// performs the shutdown; subsequent calls are no-ops. Also aborts the
    /// plugin health → diagnostics bridge task so it does not outlive the
    /// host whose events it forwards (#238).
    async fn shutdown_plugin_host(&self) {
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
            drop(self.cmd_tx.send(EneCommand::Shutdown));
            // Last handle clone: shut down the plugin host (#247). `Drop` is
            // synchronous, so spawn the async shutdown on the runtime when one
            // is available; `SupervisedPlugin::drop` kills the processes as a
            // synchronous backstop regardless.
            if Arc::strong_count(&self.plugin_host) == 1 {
                let plugin_host = Arc::clone(&self.plugin_host);
                let health_bridge_handle = Arc::clone(&self.health_bridge_handle);
                let shutdown = async move {
                    let mut guard = plugin_host.lock().await;
                    if let Some(mut host) = guard.take() {
                        host.shutdown().await;
                    }
                    drop(guard);

                    // Abort the health → diagnostics bridge so it does not
                    // outlive the host whose events it forwards (#238).
                    let mut bridge = health_bridge_handle.lock().await;
                    if let Some(handle) = bridge.take() {
                        handle.abort();
                    }
                };
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        handle.spawn(shutdown);
                    }
                    Err(_) => {
                        tracing::warn!(
                            component = "EneHandle",
                            "No tokio runtime available for plugin host shutdown; \
                             relying on process-kill backstop"
                        );
                    }
                }
            }
        }
    }
}

/// Maps a [`PluginHealthEvent`] to a [`DiagnosticEvent::ToolHealth`]
/// with a stable English status contract (#238).
///
/// Duplicated (small) from `actor::plugin_health_event_to_diag` because that
/// one is private to the `actor` module and this bootstrap-time bridge runs
/// before the actor exists; kept in lockstep by both mapping the same
/// `PluginHealthEvent` variants to the same status strings.
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
        let plugins = ene_plugin_host::PluginConfig {
            enabled: false,
            ..Default::default()
        };
        drop(config.set_section(&plugins));
        let ai = ene_ai::AiConfig::default();
        drop(config.set_section(&ai));
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
            drop(handle.event_tx.send(EneEvent::TextDelta {
                turn: TurnId::new(),
                origin: crate::types::TurnOrigin::User,
                delta: format!("delta {i}"),
            }));
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
            drop(
                handle
                    .diag_tx
                    .send(crate::diagnostics::DiagnosticEvent::PipelinePhase {
                        turn: TurnId::new(),
                        phase: format!("phase {i}"),
                    }),
            );
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

        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    #[tokio::test]
    async fn isolate_panic_returns_output_on_success() {
        let (diag_tx, _diag_rx) = broadcast::channel(16);
        let result = actor::isolate_panic(&diag_tx, "test", async { 42u32 }).await;
        assert_eq!(result.expect("no panic"), 42);
    }

    #[tokio::test]
    async fn isolate_panic_contains_panic_and_emits_diagnostic() {
        let (diag_tx, mut diag_rx) = broadcast::channel(16);
        let result: Result<u32, String> = actor::isolate_panic(&diag_tx, "test-component", async {
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

    /// Regression test for #268: with `panic = "abort"` removed from the
    /// release profile, `catch_unwind`-based isolation in
    /// `run_command_isolated` is finally live in release builds (it always
    /// unwound in debug/test, which is why this couldn't be caught by simply
    /// running the test suite before the profile fix — the risk was release
    /// builds specifically). This drives a full mailbox round trip through a
    /// live actor (unlike `isolate_panic_contains_panic_and_emits_diagnostic`,
    /// which calls the bare `isolate_panic` helper directly) and asserts all
    /// four properties #268 requires:
    ///   (a) the panic is caught, not propagated out of the actor task,
    ///   (b) the actor survives and keeps processing commands afterward,
    ///   (c) a `DiagnosticEvent::ActorPanic { component: "command", .. }` fires,
    ///   (d) `pending_permissions`, `permission_scopes`, and `undo_stack` —
    ///       the three fields #268 flagged for audit — are left in a
    ///       consistent, still-usable state: the mutations recorded just
    ///       before the panic are fully present, not torn or lost, and the
    ///       (non-poisoning) locks guarding them are not stuck.
    #[tokio::test]
    async fn actor_survives_command_panic_and_audited_state_stays_consistent() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");
        let mut diag_rx = handle.diagnostics().subscribe();

        let request_id = RequestId::new("test-268-request");
        let (permission_tx, permission_rx) = oneshot::channel();

        // Send the panic-inducing command directly through the private
        // mailbox sender (test-only `EneCommand` variant, see command.rs).
        // `cmd_tx` is an unbounded mpsc, so ordering with the `list_permissions`
        // call below is guaranteed by FIFO delivery into the single-threaded
        // actor loop, without needing a reply channel on this command itself.
        handle
            .cmd_tx
            .send(EneCommand::TestInjectPanicAfterMutations {
                request_id: request_id.clone(),
                permission_tx,
            })
            .expect("actor mailbox open");

        // (a) + (b): if the panic had propagated out of `run_command_isolated`
        // and killed the actor task, every subsequent call on `handle` would
        // return `ActorDeadError` / a dropped reply channel. It doesn't.
        let scopes = handle
            .list_permissions()
            .await
            .expect("actor must still be alive and answer ListPermissions after the panic");

        // (d) permission_scopes: the push completed before the panic and the
        // tokio::sync::Mutex guarding it was released on unwind (it does not
        // poison), so the entry is present and intact — not lost, not duplicated.
        assert_eq!(
            scopes.len(),
            1,
            "expected exactly the one scope pushed before the panic, got {scopes:?}"
        );
        assert_eq!(scopes[0].id, 999_999);
        assert_eq!(scopes[0].action, "test.action");
        assert_eq!(scopes[0].target_pattern, "test-pattern");

        // (d) pending_permissions: resolve the entry inserted before the panic
        // through the real public API. If the map had been left empty, stuck
        // locked, or the sender dropped, this would fail (ActorDeadError) or
        // `permission_rx` below would never resolve.
        handle
            .decide_permission(request_id, PermissionDecision::AllowOnce)
            .expect("actor must still accept PermissionDecision after the panic");
        let decision = permission_rx
            .await
            .expect("pending_permissions entry must have survived the panic");
        assert_eq!(decision, PermissionDecision::AllowOnce);

        // (d) undo_stack: the recorded entry from before the panic must still
        // be poppable and correctly attributed, proving the stack was not
        // left in a partially-updated state.
        let report = handle
            .undo()
            .await
            .expect("actor must still be alive and answer Undo after the panic");
        match report {
            crate::undo::UndoReport::Reverted { metadata, .. }
            | crate::undo::UndoReport::Failed { metadata, .. } => {
                // No `utility.undo` tool is registered in this test's plugin-off
                // config, so the rollback itself is expected to fail (`Failed`);
                // either way what matters here is that the *metadata* recorded
                // before the panic round-trips intact.
                assert_eq!(metadata.tool_name, "filesystem.write");
                assert_eq!(metadata.turn_id, "test-turn");
                assert_eq!(metadata.target_resources, vec!["/test/path".to_string()]);
            }
            other => panic!("expected the pre-panic undo entry to be poppable, got {other:?}"),
        }

        // (c) the panic must have been surfaced as a diagnostic, not swallowed
        // silently.
        let mut saw_panic = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let DiagnosticEvent::ActorPanic { component, message } = ev
                && component == "command"
            {
                assert!(
                    message.contains("induced panic after mutating shared actor state"),
                    "unexpected panic message: {message}"
                );
                saw_panic = true;
            }
        }
        assert!(
            saw_panic,
            "expected DiagnosticEvent::ActorPanic {{ component: \"command\", .. }}"
        );

        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    /// Regression test for #271: a read-only session query must not queue
    /// behind an in-flight `Run` turn. Before the split, `ListSessions` was
    /// an `EneCommand` variant handled by the same single-threaded actor
    /// mailbox as `Run`, so a slow/long turn would starve session queries.
    /// `SessionQueryHandle` talks to `MemoryStore` directly and must return
    /// promptly even while a turn is streaming.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn session_query_is_not_blocked_by_in_flight_turn() {
        let mut config = test_config_memory_off();
        // Point the chat provider at an address nothing answers so the turn
        // stays "in flight" (streaming/retrying) for the duration of this test.
        let mut ai = ene_ai::AiConfig::default();
        if let Some(default_provider) = ai.providers.get_mut("default") {
            default_provider.base_url = "http://127.0.0.1:1".to_string();
        }
        drop(config.set_section(&ai));

        let handle = EneHandle::open(config, test_card())
            .await
            .expect("open initializes handle");

        let turn = handle.run("hello").expect("first run starts");

        let sessions = handle.sessions();
        let started = std::time::Instant::now();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(2), sessions.list(false, 10))
                .await
                .expect("session query must return promptly, not queue behind the in-flight turn");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "session query took {elapsed:?}, expected it to bypass the turn-execution actor mailbox"
        );
        // Memory store is disabled in this config, so the query itself
        // resolves to an error — what matters here is *how fast* it resolves.
        assert!(result.is_err(), "memory store is disabled in this config");

        drop(handle.cancel(&turn));
        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    // ── Stage 8: bounded actor task admission ──

    /// A `ToolRegistry` with no tools, whose `call_tool` always succeeds.
    /// Good enough for admission-cap tests below, which only care about how
    /// many times a `JoinSet` was asked to admit a task, not about real tool
    /// behavior.
    struct EmptyRegistry;

    #[async_trait::async_trait]
    impl ene_plugin_host::ToolRegistry for EmptyRegistry {
        fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
            Vec::new()
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: &str,
            _context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, ene_plugin_host::PluginHostError> {
            Ok(ene_plugin_proto::ToolResult::text("done"))
        }
    }

    /// Builds a bare `TurnActor` directly, bypassing `EneHandle::open`'s
    /// plugin-host / DB / embedder bootstrapping entirely, so admission-cap
    /// tests can drive [`actor::TurnActor::handle_command`] one call at a
    /// time with no `run()` loop (and therefore no `reap_join_set` calls)
    /// racing in between. That matters because a `JoinSet`'s `len()` only
    /// ever shrinks when something reaps it; calling `handle_command`
    /// directly means the length seen by `admit_task` across two successive
    /// calls in a test is fully deterministic regardless of how fast the
    /// mock tool call actually completes.
    fn build_bare_actor(
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        task_caps: &crate::task_config::ToolRuntimeConfig,
    ) -> (actor::TurnActor, broadcast::Receiver<DiagnosticEvent>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, _event_rx) = broadcast::channel(16);
        let (lifecycle_tx, _lifecycle_rx) = broadcast::channel(16);
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let (diag_tx, diag_rx) = broadcast::channel(64);
        let mut config = EneConfig::default();
        config
            .set_section(task_caps)
            .expect("set_section(ToolRuntimeConfig) succeeds");
        let health_monitor =
            ene_ai::ProviderHealthMonitor::new(std::time::Duration::from_secs(1), 8);
        let actor = actor::TurnActor::new(
            cmd_rx,
            cmd_tx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            diag_tx.clone(),
            diag_tx.subscribe(),
            Arc::new(TurnGate::new()),
            config,
            ConversationSession::new(),
            registry,
            None,
            health_monitor,
            None,
            Vec::new(),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(parking_lot::Mutex::new(String::new())),
        );
        (actor, diag_rx)
    }

    /// Pure test of the shared admission mechanism (`actor::admit_task`),
    /// used identically by all five of `TurnActor`'s bounded `JoinSet`s:
    /// under cap it must allow admission silently, and right at cap it must
    /// reject and emit a `TaskRejected` diagnostic with the offending
    /// component/cap/detail. Covers the classifier / memory-writer /
    /// deferred-tool drain sites (which have no reply channel of their own)
    /// by exercising the exact function they all call.
    #[tokio::test]
    async fn admit_task_allows_up_to_cap_then_rejects_with_diagnostic() {
        let (diag_tx, mut diag_rx) = broadcast::channel(16);
        let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

        assert!(actor::admit_task(&set, 2, "TestSet", None, &diag_tx));
        set.spawn(async {});
        assert!(actor::admit_task(&set, 2, "TestSet", None, &diag_tx));
        set.spawn(async {});
        assert!(!actor::admit_task(
            &set,
            2,
            "TestSet",
            Some("detail".to_string()),
            &diag_tx
        ));

        let ev = diag_rx
            .try_recv()
            .expect("expected a TaskRejected diagnostic");
        match ev {
            DiagnosticEvent::TaskRejected {
                component,
                cap,
                detail,
            } => {
                assert_eq!(component, "TestSet");
                assert_eq!(cap, 2);
                assert_eq!(detail.as_deref(), Some("detail"));
            }
            other => panic!("expected TaskRejected, got {other:?}"),
        }
    }

    /// End-to-end (through `EneCommand::CallTool`, not just the bare
    /// `admit_task` helper): with `call_tool_cap` set to 1, a second
    /// `CallTool` sent while the first is still occupying the `JoinSet`'s
    /// only slot must be rejected with `EneRuntimeError::Busy` on its own
    /// reply channel *and* a `TaskRejected` diagnostic, while the first call
    /// is admitted and completes normally.
    #[tokio::test]
    async fn call_tool_admission_rejects_busy_once_at_cap() {
        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig {
            call_tool_cap: 1,
            ..Default::default()
        };
        let (mut actor, mut diag_rx) = build_bare_actor(registry, &task_caps);

        let (tx1, rx1) = oneshot::channel();
        assert!(
            actor
                .handle_command(EneCommand::CallTool {
                    name: "anything".into(),
                    arguments: "{}".into(),
                    turn: None,
                    reply: tx1,
                })
                .await,
            "handle_command must not signal shutdown"
        );

        let (tx2, rx2) = oneshot::channel();
        assert!(
            actor
                .handle_command(EneCommand::CallTool {
                    name: "anything".into(),
                    arguments: "{}".into(),
                    turn: None,
                    reply: tx2,
                })
                .await
        );

        let res2 = rx2.await.expect("reply channel not dropped");
        assert!(
            matches!(res2, Err(EneRuntimeError::Busy { queue_depth: 1 })),
            "expected Busy{{queue_depth:1}} for the second call, got {res2:?}"
        );

        let mut saw_rejected = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let DiagnosticEvent::TaskRejected { component, cap, .. } = ev
                && component == "CallTool"
                && cap == 1
            {
                saw_rejected = true;
            }
        }
        assert!(
            saw_rejected,
            "expected a TaskRejected diagnostic for CallTool"
        );

        let res1 = rx1.await.expect("reply channel not dropped");
        assert_eq!(
            res1.expect("first call must be admitted and succeed"),
            "done"
        );
    }

    /// Same shape as the `CallTool` test above, for `EneCommand::SearchTools`
    /// and `search_cap`.
    #[tokio::test]
    async fn search_tools_admission_rejects_busy_once_at_cap() {
        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(EmptyRegistry);
        let task_caps = crate::task_config::ToolRuntimeConfig {
            search_cap: 1,
            ..Default::default()
        };
        let (mut actor, mut diag_rx) = build_bare_actor(registry, &task_caps);

        let (tx1, rx1) = oneshot::channel();
        assert!(
            actor
                .handle_command(EneCommand::SearchTools {
                    query: "anything".into(),
                    reply: tx1,
                })
                .await
        );

        let (tx2, rx2) = oneshot::channel();
        assert!(
            actor
                .handle_command(EneCommand::SearchTools {
                    query: "anything".into(),
                    reply: tx2,
                })
                .await
        );

        let res2 = rx2.await.expect("reply channel not dropped");
        assert!(
            matches!(res2, Err(EneRuntimeError::Busy { queue_depth: 1 })),
            "expected Busy{{queue_depth:1}} for the second search, got {res2:?}"
        );

        let mut saw_rejected = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let DiagnosticEvent::TaskRejected { component, cap, .. } = ev
                && component == "SearchTools"
                && cap == 1
            {
                saw_rejected = true;
            }
        }
        assert!(
            saw_rejected,
            "expected a TaskRejected diagnostic for SearchTools"
        );

        let res1 = rx1.await.expect("reply channel not dropped");
        assert!(
            res1.expect("first search must be admitted and succeed")
                .is_empty(),
            "expected the empty registry's tool list to filter to nothing"
        );
    }

    // ── #397: no head-of-line blocking in the command loop ──

    /// Regression test for #397: a heavy command whose work has been moved
    /// off the actor loop into `bg_command_tasks` must not block subsequent
    /// commands. Before the fix, handlers such as the GGUF model load and
    /// the plugin-host restart awaited their heavy I/O *inline*, stalling the
    /// entire mailbox (including `Cancel`) for the duration.
    ///
    /// This drives a live actor through its real `run()` loop: it occupies a
    /// `bg_command_tasks` slot with a long-sleeping task (simulating an
    /// in-flight heavy command), then asserts a follow-up command is still
    /// answered promptly rather than queueing behind the background work.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn command_loop_not_blocked_by_in_flight_bg_task() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");

        // Occupy a background slot with a slow task, and wait until the actor
        // confirms it has been spawned. `cmd_tx` is an unbounded FIFO mpsc, so
        // the reply arriving guarantees the slot is occupied before we proceed.
        let (spawned_tx, spawned_rx) = oneshot::channel();
        handle
            .cmd_tx
            .send(EneCommand::TestSpawnSlowBgTask { reply: spawned_tx })
            .expect("actor mailbox open");
        spawned_rx.await.expect("slow bg task spawn acknowledged");

        // A follow-up command must be processed promptly even though the
        // background task is still "in flight". If the actor loop were
        // head-of-line blocked on the heavy work, this would not return until
        // that work finished (30s here) and the tight timeout would fire.
        let started = std::time::Instant::now();
        let scopes =
            tokio::time::timeout(std::time::Duration::from_secs(2), handle.list_permissions())
                .await
                .expect("follow-up command must not queue behind an in-flight bg task")
                .expect("actor must still be alive and answer ListPermissions");
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "follow-up command took {elapsed:?}; the actor loop appears head-of-line blocked"
        );
        assert!(
            scopes.is_empty(),
            "no permissions granted in this test config"
        );

        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }
}
