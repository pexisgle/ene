//! Actor-based runtime with message-passing architecture.
//!
//! [`EneHandle`] is the thread-safe facade constructed by [`EneHandle::open`].
//! It owns the command channel into the single-threaded [`actor::TurnActor`]
//! (turn execution, permissions, undo, tool calls, character/config control)
//! and delegates read-only session/candidate queries and screen-image vision
//! summarization to dedicated handles that bypass the actor mailbox entirely
//! — see [`EneHandle::sessions`], [`EneHandle::candidates`], and
//! [`EneHandle::vision`]. Tool operations get their own handle via
//! [`EneHandle::tools`]; unlike the read-only handles it still routes
//! through the mailbox — see [`crate::tools`] for why.
//!
//! Small per-frame state (card name, session id, turn count, config, loaded
//! card) is additionally mirrored into a mailbox-free [`SharedActorState`]
//! the actor keeps in sync at the mutation points — see
//! [`EneHandle::card_name`], [`EneHandle::session_id`],
//! [`EneHandle::turn_count`], [`EneHandle::config`], and
//! [`EneHandle::character_card`]. Only the large history payload stays
//! mailbox-based ([`EneHandle::history`]).
//!
//! ## Module layout
//!
//! - [`command`] — [`EneCommand`] (turn-execution / control-plane commands).
//! - [`event`] — [`EneEvent`] (chat bus), [`AudioChunk`] (audio channel),
//!   [`LifecycleEvent`] (lifecycle bus), [`TerminalReason`], [`EneStatus`],
//!   [`EneEventReceiver`], [`AudioStreamReceiver`], [`LifecycleReceiver`],
//!   [`EneStateSnapshot`]. See the module doc there for the three-channel
//!   event bus design.
//! - [`actor`] — [`actor::TurnActor`] and its
//!   supporting free functions.
//! - [`crate::query::sessions`] / [`crate::query::candidates`] — read-only
//!   session and pending-candidate handles.
//! - [`crate::vision`] — screen-image vision summarization handle.
//! - [`crate::tools`] — tool registry handle (list / search / call /
//!   invalidate).

mod actor;
mod command;
mod event;

/// Re-exported so `ene_runtime::handle::PendingCandidateSummary` (the path
/// some embedders import) keeps resolving; the type lives in
/// [`crate::query::candidates`].
pub use crate::query::candidates::PendingCandidateSummary;
pub use command::{DeferredToolTask, EneCommand, FeatureSettingsUpdate};
pub use event::{
    AudioChunk, AudioStreamReceiver, EneEvent, EneEventReceiver, EneStateSnapshot, EneStatus,
    LifecycleEvent, LifecycleReceiver, TerminalReason,
};

use crate::diagnostics::{DiagnosticEvent, emit_diag};
use crate::error::EneRuntimeError;
use crate::public_api::PublicApiError;
use crate::streaming::{PermissionDecision, UserInputResponse};
use crate::types::{CancelError, RequestId, RunError, TurnId};
use chrono::{DateTime, Utc};
use ene_config::CharacterCardV3;
use ene_config::EneConfig;
use ene_mind::{ConversationSession, SessionId};
use ene_plugin_host::{DisabledReason, PluginHealthEvent};
use ene_rag::ToolRagConfig;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

/// Bounded capacity for the dedicated audio (PCM) channel.
///
/// Chosen to buffer several seconds of TTS chunks (each chunk is roughly one
/// sentence of synthesized speech) ahead of a real-time playback consumer
/// without letting an unconsumed stream grow without bound. See
/// `send_audio_chunk` in `streaming_cognitive.rs` for the back-pressure
/// policy applied when this fills up.
const AUDIO_CHANNEL_CAPACITY: usize = 64;

/// Small broadcast capacity for the lifecycle bus.
///
/// Lifecycle notifications (`StatusChanged`, `PendingCandidateAvailable`,
/// `ToolBackgroundCompleted`) are low-frequency and turn-independent, so a
/// much smaller buffer than the chat bus's 1024 is sufficient.
const LIFECYCLE_CHANNEL_CAPACITY: usize = 64;

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

/// RAII guard that triggers graceful shutdown when the last handle clone drops.
///
/// Each [`EneHandle`] and its clones share a single `Arc<HandleShutdownGuard>`.
/// When the last clone of `EneHandle` is dropped, the guard's `Drop` sends
/// [`EneCommand::Shutdown`] and spawns async plugin-host shutdown on the
/// current tokio runtime.
///
/// This guard exists because the `Arc::strong_count(&cmd_tx) == 1`
/// approach is unreliable: `EneHandle` internally holds four independent
/// `Arc` clones of `cmd_tx` (self, diagnostics, vision, tools), so the count
/// never reaches 1 while the handle is alive; the shutdown guard holds a bare
/// sender clone.
///
/// ## Drop shutdown is best-effort
///
/// `Drop` is synchronous, so it can only *initiate* shutdown, not await it:
///
/// - The actor may still be running after `Drop` returns — the guard sends
///   [`EneCommand::Shutdown`] but the actor drains asynchronously. Background
///   tasks spawned by the actor (classifiers, memory writers, deferred tool
///   tasks) become detached tokio tasks.
/// - The plugin-host shutdown is spawned onto the current runtime via
///   [`tokio::runtime::Handle::try_current`]. If the last handle drops during
///   process teardown — no live runtime, or one that is already shutting
///   down — the spawned task may never run to completion. The plugin host's
///   `SupervisedPlugin::drop` killing and reaping the child processes is the
///   synchronous backstop for that case.
///
/// Callers that need a clean shutdown guarantee should call
/// [`EneHandle::shutdown`] with an explicit timeout while the runtime is
/// still alive, before dropping the handle.
struct HandleShutdownGuard {
    /// Bare sender clone for the shutdown command. The guard holds its own
    /// clone so it can send even after all `EneHandle` fields are dropped.
    cmd_tx: mpsc::UnboundedSender<EneCommand>,
    /// Keeps the actor's `JoinHandle` alive until after the guard sends
    /// `Shutdown`, preventing premature task detach when `EneHandle`'s
    /// own `actor_handle` field (declared earlier) drops first.
    #[expect(
        dead_code,
        reason = "held to keep the Arc<Mutex<Option<JoinHandle>>> alive until \
                  after Shutdown is sent; not read directly in Drop"
    )]
    actor_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl Drop for HandleShutdownGuard {
    fn drop(&mut self) {
        tracing::info!(
            component = "EneHandle",
            "Last handle dropped; initiating graceful shutdown"
        );

        // Send shutdown command to the actor. If the channel is closed
        // (actor already exited or explicit shutdown consumed it), this
        // is a silent no-op.
        drop(self.cmd_tx.send(EneCommand::Shutdown));

        // Shut down the plugin host and abort the health bridge. `Drop`
        // is synchronous, so spawn on the tokio runtime when one is
        // available; `SupervisedPlugin::drop` kills the child processes
        // as a synchronous backstop regardless.
        let plugin_host = Arc::clone(&self.plugin_host);
        let health_bridge_handle = Arc::clone(&self.health_bridge_handle);
        let host_service_handle = Arc::clone(&self.host_service_handle);
        let shutdown = async move {
            let mut guard = plugin_host.lock().await;
            if let Some(mut host) = guard.take() {
                host.shutdown().await;
            }
            drop(guard);

            // Abort the health → diagnostics bridge so it does not
            // outlive the host whose events it forwards.
            let mut bridge = health_bridge_handle.lock().await;
            if let Some(handle) = bridge.take() {
                handle.abort();
            }

            // Abort the host-service accept loop so it does not outlive the
            // process that owns its socket.
            let mut acceptor = host_service_handle.lock().await;
            if let Some(handle) = acceptor.take() {
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

/// Cross-slot-atomic session snapshot published by the actor.
///
/// `session_id`, `session_started_at`, and `turn_count` are replaced together
/// under a single write lock so a reader never observes a new session id with
/// a stale turn count (or any other mid-split mix).
#[derive(Clone, Debug)]
pub(crate) struct SharedSessionState {
    pub(crate) session_id: SessionId,
    pub(crate) session_started_at: DateTime<Utc>,
    pub(crate) turn_count: u32,
}

/// Mailbox-free shared actor state, kept in sync by the turn actor.
///
/// Every field is a shared slot written by the actor at the mutation point —
/// session split ([`actor::TurnActor`]'s `split_session`), `SetCharacter`,
/// per-turn bookkeeping (`record_user_input` / the stream outcome), and
/// feature-settings updates — and read synchronously by [`EneHandle`]'s
/// lightweight accessors ([`EneHandle::card_name`],
/// [`EneHandle::session_id`], [`EneHandle::session_started_at`],
/// [`EneHandle::turn_count`], [`EneHandle::config`],
/// [`EneHandle::character_card`]). No mailbox round-trip is involved, so the
/// reads are safe to call from egui immediate mode and never queue behind an
/// in-flight `Run` turn.
///
/// `card_name` is the precedent: it already shared an
/// `Arc<Mutex<String>>` between the actor and
/// [`crate::query::candidates::MemoryCandidateHandle`]; the other slots
/// extend the same pattern to the rest of the snapshot-ish surface instead
/// of duplicating state.
#[derive(Clone)]
pub(crate) struct SharedActorState {
    /// Current character-card name, kept in sync on every
    /// `SetCharacter`.
    pub(crate) card_name: Arc<parking_lot::Mutex<String>>,
    /// Current session id / started-at / turn count, replaced atomically on
    /// session split and after each recorded turn.
    pub(crate) session: Arc<parking_lot::RwLock<SharedSessionState>>,
    /// Current configuration. `RwLock` (readers hold a shared lock and clone
    /// the `Arc`; the actor swaps it under a write lock) so per-frame reads
    /// never serialize against each other — an "ArcSwap-style" sharing
    /// pattern without the dependency.
    pub(crate) config: Arc<parking_lot::RwLock<Arc<EneConfig>>>,
    /// Loaded character card, replaced on `SetCharacter`.
    pub(crate) character_card: Arc<parking_lot::Mutex<Option<Arc<CharacterCardV3>>>>,
}

impl SharedActorState {
    /// Builds the shared slots from the bootstrap config and session. Called
    /// once from [`EneHandle::open`]; the actor keeps the slots in sync from
    /// then on.
    fn new(config: &EneConfig, session: &ConversationSession) -> Self {
        Self {
            card_name: Arc::new(parking_lot::Mutex::new(session.card_name().to_string())),
            session: Arc::new(parking_lot::RwLock::new(SharedSessionState {
                session_id: session.memory.session_id.clone(),
                session_started_at: session.session_started_at(),
                turn_count: session.current_turn_count() as u32,
            })),
            config: Arc::new(parking_lot::RwLock::new(Arc::new(config.clone()))),
            character_card: Arc::new(parking_lot::Mutex::new(
                session.character_card.clone().map(Arc::new),
            )),
        }
    }
}

/// Thread-safe handle to the ready actor.
///
/// Constructed only via [`EneHandle::open`], which initializes provider, store,
/// tools, mind session, and warmup before returning. When the last clone is
/// dropped the underlying `mpsc` channel closes and the actor exits.
pub struct EneHandle {
    /// Command mailbox sender. Deliberately an *unbounded* `mpsc` — see the
    /// rationale at the channel's construction in [`EneHandle::open`]:
    /// it is shared with the actor's internal feedback path and the
    /// drop-guard `Shutdown`, neither of which may be dropped by backpressure.
    /// Expensive work is bounded at `JoinSet` admission, not here.
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    /// Mailbox-free shared actor state (card name, session id, turn count,
    /// config, character card) kept in sync by the actor. Read via the
    /// lightweight accessors below; large payloads (history) still go through
    /// the mailbox via [`EneHandle::history`].
    shared: Arc<SharedActorState>,
    event_tx: broadcast::Sender<EneEvent>,
    /// Lifecycle bus sender: `StatusChanged` / `PendingCandidateAvailable`
    /// / `ToolBackgroundCompleted`. Separate broadcast channel from `event_tx`
    /// so lifecycle traffic never shares a buffer with chat events.
    lifecycle_tx: broadcast::Sender<LifecycleEvent>,
    /// Audio channel sender: bounded `mpsc`, single-consumer. The
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
    /// graceful plugin shutdown. `None` when plugin startup failed
    /// or no plugins were discovered.
    plugin_host: Arc<tokio::sync::Mutex<Option<ene_plugin_host::PluginHostManager>>>,
    /// Join handle for the plugin health → diagnostics bridge task.
    ///
    /// Shared across handle clones. Aborted during plugin host shutdown so
    /// the bridge does not outlive the host whose events it forwards.
    health_bridge_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Join handle for the host-service accept loop.
    ///
    /// Shared across handle clones. Aborted during shutdown so the accept
    /// loop does not outlive the process that owns its socket.
    host_service_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Read-only session query handle, bypasses the actor mailbox.
    sessions: crate::query::sessions::SessionQueryHandle,
    /// Pending memory-candidate approval handle. Reads bypass the actor
    /// mailbox; mutations route through it (see
    /// [`crate::query::candidates::MemoryCandidateHandle`]).
    candidates: crate::query::candidates::MemoryCandidateHandle,
    /// Screen-image vision summarization handle, bypasses the actor mailbox.
    vision: crate::vision::VisionHandle,
    /// Tool registry operations handle (list / search / call / invalidate).
    /// Routes through the actor mailbox — see [`crate::tools`].
    tools: crate::tools::ToolHandle,
    /// RAII guard that triggers graceful shutdown when the last handle clone
    /// drops. Shared via `Arc` across all handle clones so the guard's `Drop`
    /// fires exactly once — when the last clone is released. Declared as the
    /// last field so all command-sending fields (`cmd_tx`, `diagnostics`,
    /// `vision`) are dropped before it; the guard holds its own sender clone,
    /// so field order is not critical for correctness but makes intent clear.
    ///
    /// Drop-initiated shutdown is best-effort (see [`HandleShutdownGuard`]):
    /// callers needing a hard guarantee should call [`EneHandle::shutdown`]
    /// before dropping the handle.
    shutdown_guard: Arc<HandleShutdownGuard>,
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
            shared: Arc::clone(&self.shared),
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
            host_service_handle: Arc::clone(&self.host_service_handle),
            sessions: self.sessions.clone(),
            candidates: self.candidates.clone(),
            vision: self.vision.clone(),
            tools: self.tools.clone(),
            shutdown_guard: Arc::clone(&self.shutdown_guard),
        }
    }
}

impl EneHandle {
    /// Open a ready runtime handle.
    ///
    /// Initializes the embedding provider, memory store (when enabled), tool
    /// registry, mind session with `card`, and character memory warmup
    /// **before** returning `Ok`. Provider creation resolves through the
    /// plugin host's registry. Config file I/O stays in the host /
    /// `ene-config` — pass an already-loaded config and card.
    pub async fn open(config: EneConfig, card: CharacterCardV3) -> Result<Self, EneRuntimeError> {
        let plugin_host_slot: crate::provider_host::PluginHostSlot = Arc::new(Mutex::new(None));
        let provider_host: Arc<dyn ene_ai::ProviderHost> = Arc::new(
            crate::provider_host::LiveProviderCatalog::new(Arc::clone(&plugin_host_slot)),
        );
        Self::open_inner(config, card, provider_host, plugin_host_slot).await
    }

    /// Open a ready runtime handle with an injected provider host.
    ///
    /// Provider creation (LLM/embedding/TTS) routes through `provider_host`
    /// instead of the live plugin host. Used by tests that stand in for the
    /// plugin registry with stub factories; the plugin host is still started
    /// (for tools) unless the config disables it.
    pub async fn open_with_provider_host(
        config: EneConfig,
        card: CharacterCardV3,
        provider_host: Arc<dyn ene_ai::ProviderHost>,
    ) -> Result<Self, EneRuntimeError> {
        Self::open_inner(config, card, provider_host, Arc::new(Mutex::new(None))).await
    }

    async fn open_inner(
        config: EneConfig,
        card: CharacterCardV3,
        provider_host: Arc<dyn ene_ai::ProviderHost>,
        plugin_host_slot: crate::provider_host::PluginHostSlot,
    ) -> Result<Self, EneRuntimeError> {
        // The command mailbox is deliberately an *unbounded* `mpsc`.
        //
        // Stage 8 bounded the five background `JoinSet`s (admission control),
        // but the channel feeding them was left unbounded. Bounding it here
        // was considered and rejected for this shared channel:
        //
        // - It is shared by three very different producers: external consumers
        //   (UI / CLI), the actor's own background tasks (which feed
        //   `PluginHostReconfigured` back through it), and the
        //   [`HandleShutdownGuard`] drop path (which sends `Shutdown` from a
        //   synchronous `Drop`). A bounded channel with `try_send` semantics
        //   would let a full mailbox silently drop the shutdown command or an
        //   internal reconfiguration — both correctness bugs, not backpressure.
        // - The only externally floodable command is
        //   [`EneCommand::UpdateProactiveObservation`], pushed once per screen
        //   capture by the desktop proactive observer. That producer is
        //   rate-limited at its source (capture cadence), not by the mailbox,
        //   so the realistic flood vector is already bounded upstream.
        // - Backpressure for the genuinely expensive work (tool calls,
        //   searches, deferred pollers, GGUF loads) is enforced where it
        //   matters — at `JoinSet` admission — and fails fast with
        //   [`crate::error::EneRuntimeError::Busy`] rather than queueing.
        //
        // The trade-off is documented rather than enforced: a misbehaving
        // consumer that loops `update_proactive_observation` without awaiting
        // the actor can grow this mailbox. That is a caller bug, surfaced by
        // the actor's own throughput, not a condition the runtime masks.
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let cmd_tx = Arc::new(cmd_tx);
        let (event_tx, _event_rx) = broadcast::channel(1024);
        let (lifecycle_tx, _lifecycle_rx) = broadcast::channel(LIFECYCLE_CHANNEL_CAPACITY);
        let (audio_tx, audio_rx) = mpsc::channel(AUDIO_CHANNEL_CAPACITY);
        let (diag_tx, diag_rx) = broadcast::channel(256);

        // Register the local whisper/kokoro/silero factories with
        // `ene_ai::AudioProviderRegistry` before anything below (this
        // function's own TTS provider resolution, or a later
        // caller-initiated STT/VAD lookup such as the desktop app's mic
        // toggle) can look one up by name. Doing it here, after `tracing`
        // is up, makes registration observable (a `#[ctor::ctor]` would
        // run before `main` and before `tracing` was initialized).
        ene_voice::register_providers();

        let mind = config.get_section::<ene_mind::MindConfig>()?;

        // Startup validation: warn on mind-config timing relationships that
        // are accepted but cannot fire sooner than the next poll tick.
        ene_mind::warn_on_proactive_interval_issues(&mind.proactive);

        // Startup validation: warn when a configured context window is
        // too small for the prompt budget plus output reserve, since prompt
        // sections would otherwise be silently dropped every turn.
        // `max_prompt_tokens` is an *optional* operator cap: with no cap the
        // prompt auto-follows the model's effective window, so there is no
        // fixed budget to validate against and nothing to warn about.
        if let Some(cap) = mind.context.max_prompt_tokens
            && let Ok(cap) = u32::try_from(cap)
        {
            let ai_config = config.get_section::<ene_ai::AiConfig>()?;
            ene_ai::warn_on_context_budget_issues(&ai_config, cap);
        }

        let mem_config = config.get_section::<ene_store::StoreConfig>()?;
        let plugin_config = config.get_section::<ene_plugin_host::PluginConfig>()?;
        let rag_config = config.get_section::<ToolRagConfig>()?;
        let needs_embedder = mem_config.enabled || (plugin_config.enabled && rag_config.enabled);

        // Fail-closed: memory / tool-RAG features require a working embedder.
        let embedder = if needs_embedder {
            Some(actor::init_embedding(&config, Arc::clone(&provider_host))?)
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

        let (db_tokens, host_service_handle) =
            actor::spawn_db_ipc_servers(&config, memory_store.as_ref())?;

        // Start the plugin host (discovers and launches v3 plugin binaries).
        // Non-fatal: on failure we log and continue with no plugin-provided
        // providers/tools, mirroring the tool host's empty-set fallback. The
        // host itself is the provider registry — nothing is copied out of it.
        let mut plugin_host =
            match ene_plugin_host::PluginHostManager::start(&config, db_tokens).await {
                Ok(host) => {
                    tracing::info!(
                        component = "Bootstrap",
                        tool_registries = host.tool_registries().len(),
                        llm_factories = host.llm_factories().len(),
                        embedding_factories = host.embedding_factories().len(),
                        tts_factories = host.tts_factories().len(),
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

        // Bridge plugin health events into the diagnostics channel.
        // The task's `JoinHandle` is retained so it can be aborted during
        // plugin host shutdown rather than leaking past the host's lifetime.
        let mut health_bridge_handle: Option<tokio::task::JoinHandle<()>> = None;
        if let Some(host) = plugin_host.as_mut()
            && let Some(mut health_rx) = host.take_health_receiver()
        {
            let diag_tx = diag_tx.clone();
            let cmd_tx = Arc::clone(&cmd_tx);
            health_bridge_handle = Some(tokio::spawn(async move {
                while let Some(event) = health_rx.recv().await {
                    if let PluginHealthEvent::Disabled { plugin, .. } = &event {
                        drop(cmd_tx.send(EneCommand::PluginProviderDisabled {
                            plugin: plugin.clone(),
                        }));
                    }
                    emit_diag(&diag_tx, plugin_health_event_to_diag(event));
                }
            }));
        }

        let registry = actor::build_tool_registry(plugin_host.as_ref())?;
        let tool_rag = match embedder.as_ref() {
            Some(emb) => actor::init_tool_rag(&config, emb, memory_store.clone())?,
            None => None,
        };
        let workspace_indexer = match (embedder.as_ref(), memory_store.clone()) {
            (Some(emb), Some(store)) => {
                let store_port: Arc<dyn ene_core::WorkspaceDocumentPort> = store;
                Some(Arc::new(crate::workspace::WorkspaceIndexer::new(
                    store_port,
                    emb.clone(),
                )))
            }
            _ => None,
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

        let warmup_hash = actor::warmup_character_memories_ready(&config, &session).await;

        if let Some(hash) = warmup_hash {
            session.memory.ccv3_memory_hash = Some(hash);
        }

        let mind_memory = mind.memory.clone();
        let recall_cache = session.memory.recall_cache.clone();
        let memory = crate::diagnostics::MemoryHandle::new(
            memory_store.clone(),
            session.memory.embedding_provider.clone(),
            mind_memory,
            recall_cache.clone(),
        );

        let fallback_cfg = config.get_section::<ene_ai::AiConfig>()?.fallback;
        let health_monitor = ene_ai::ProviderHealthMonitor::new(
            std::time::Duration::from_millis(fallback_cfg.cache_ttl_ms),
            fallback_cfg.max_history,
        );

        let tts_provider = {
            let ai_config = config.get_section::<ene_ai::AiConfig>()?;

            // Prefetch the configured local TTS model files before
            // constructing the provider below, so `LocalTtsProvider::open`
            // (ene-voice) never needs to perform network I/O itself — it
            // only fails fast if a file is still missing. Mirrors the GGUF
            // prefetch above. Non-fatal: on failure we log and let provider
            // construction report a clear error.
            if let Err(e) = ene_voice::prefetch_if_configured(&config).await {
                tracing::warn!(
                    component = "Bootstrap",
                    error = %e,
                    "Local TTS model prefetch failed; will report a clear error on provider construction"
                );
            }

            if let Some(resolved) = ai_config.resolve_tts() {
                match resolve_tts_provider(provider_host.as_ref(), &config, &resolved.provider)
                    .await
                {
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
        *plugin_host_slot.lock().await = plugin_host;
        let plugin_host = plugin_host_slot;
        let health_bridge_handle = Arc::new(tokio::sync::Mutex::new(health_bridge_handle));
        let host_service_handle = Arc::new(tokio::sync::Mutex::new(host_service_handle));

        // Mailbox-free shared actor state: card name (shared with
        // `MemoryCandidateHandle` so pending-candidate queries
        // never need a mailbox round-trip), session id / started-at / turn
        // count, config, and the loaded card. The actor keeps the slots in
        // sync at the mutation points; `EneHandle` reads them synchronously.
        let shared = Arc::new(SharedActorState::new(&config, &session));

        let sessions = crate::query::sessions::SessionQueryHandle::new(memory_store.clone());
        let candidates = crate::query::candidates::MemoryCandidateHandle::new(
            memory_store.clone(),
            Arc::clone(&cmd_tx),
            Arc::clone(&shared.card_name),
        );
        let vision = crate::vision::VisionHandle::new(Arc::clone(&cmd_tx));
        let tools = crate::tools::ToolHandle::new(Arc::clone(&cmd_tx));

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
            memory_store,
            registry,
            tool_rag,
            workspace_indexer,
            health_monitor,
            tts_provider,
            plugin_tool_registries,
            provider_host,
            Arc::clone(&plugin_host),
            Arc::clone(&health_bridge_handle),
            Arc::clone(&host_service_handle),
            Arc::clone(&shared),
        );
        let join = tokio::spawn(actor.run());

        let actor_handle = Arc::new(tokio::sync::Mutex::new(Some(join)));
        let shutdown_guard = Arc::new(HandleShutdownGuard {
            cmd_tx: (*cmd_tx).clone(),
            actor_handle: Arc::clone(&actor_handle),
            plugin_host: Arc::clone(&plugin_host),
            health_bridge_handle: Arc::clone(&health_bridge_handle),
            host_service_handle: Arc::clone(&host_service_handle),
        });

        Ok(Self {
            cmd_tx,
            shared,
            event_tx,
            lifecycle_tx,
            audio_tx,
            audio_rx: Arc::new(parking_lot::Mutex::new(Some(audio_rx))),
            diag_tx,
            diagnostics,
            turn_gate,
            actor_handle,
            plugin_host,
            health_bridge_handle,
            host_service_handle,
            sessions,
            candidates,
            vision,
            tools,
            shutdown_guard,
        })
    }

    /// Subscribe to the chat event stream.
    pub fn subscribe(&self) -> EneEventReceiver {
        EneEventReceiver {
            inner: self.event_tx.subscribe(),
            diag_tx: self.diag_tx.clone(),
        }
    }

    /// Subscribe to the lifecycle event stream: `StatusChanged`,
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

    /// Take ownership of the audio (PCM) stream.
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

    /// Opt-in diagnostics facade: pipeline detail, provider health, memory.
    ///
    /// Observability only — the control surface (character swap,
    /// compression, tools) lives on this handle via [`EneHandle::set_character`],
    /// [`EneHandle::compress_context`], and [`EneHandle::tools`].
    pub const fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics {
        &self.diagnostics
    }

    /// Current stable character-card identity.
    ///
    /// Mailbox-free: reads the shared card-name slot the actor keeps in sync
    /// on every `SetCharacter` (the same slot
    /// [`crate::query::candidates::MemoryCandidateHandle`] uses). One
    /// `parking_lot` lock, no actor round-trip — safe to call from egui
    /// immediate mode.
    pub fn card_name(&self) -> String {
        self.shared.card_name.lock().clone()
    }

    /// Current session id.
    ///
    /// Mailbox-free: reads the shared session snapshot the actor replaces
    /// atomically when a session split resets the session (together with
    /// started-at and turn count). One `parking_lot` read lock, no actor
    /// round-trip.
    pub fn session_id(&self) -> SessionId {
        self.shared.session.read().session_id.clone()
    }

    /// When the current session started (UTC).
    ///
    /// Mailbox-free, same shared session snapshot as [`EneHandle::session_id`];
    /// replaced atomically on session split.
    pub fn session_started_at(&self) -> DateTime<Utc> {
        self.shared.session.read().session_started_at
    }

    /// Turn count of the current session.
    ///
    /// Mailbox-free: reads the shared session snapshot the actor updates after
    /// each recorded user input and completed stream outcome, and resets to
    /// zero on a session split (atomically with session id / started-at).
    pub fn turn_count(&self) -> u32 {
        self.shared.session.read().turn_count
    }

    /// The current configuration.
    ///
    /// Mailbox-free: clones the shared `Arc<EneConfig>` under a
    /// `parking_lot` read lock (readers never serialize against each other).
    /// The actor swaps the `Arc` under a write lock when a feature-settings
    /// update or proactive-settings update changes the config — read-only
    /// sharing here, consistent with the write-path fixes.
    pub fn config(&self) -> Arc<EneConfig> {
        self.shared.config.read().clone()
    }

    /// The loaded character card, if any.
    ///
    /// Mailbox-free: reads the shared slot the actor replaces on every
    /// `SetCharacter`. Returns `None` only if no card was ever loaded (the
    /// actor always loads the bootstrap card, so this is `Some` in practice).
    pub fn character_card(&self) -> Option<Arc<CharacterCardV3>> {
        self.shared.character_card.lock().clone()
    }

    /// Fetch the full conversation history.
    ///
    /// The one deliberately mailbox-based read: history is a large payload
    /// (`Vec<ene_mind::HistoryEntry>`), so it is *not* mirrored into the
    /// mailbox-free shared state. Use the synchronous accessors
    /// ([`EneHandle::card_name`], [`EneHandle::turn_count`], ...) for the
    /// small per-frame state and reserve this for history re-sync / bulk
    /// dumps.
    ///
    /// # Errors
    ///
    /// Returns [`PublicApiError::ActorDead`] if the actor is no longer
    /// running.
    pub async fn history(&self) -> Result<Vec<ene_mind::HistoryEntry>, PublicApiError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::GetHistory { reply: tx })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Read-only session query handle (list / export / import / search /
    /// archive), bypassing the turn-execution actor mailbox entirely.
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn sessions(&self) -> crate::query::sessions::SessionQueryHandle {
        self.sessions.clone()
    }

    /// Pending memory-candidate approval handle (list / inspect / history /
    /// approve / edit / reject).
    ///
    /// Reads are mailbox-free; mutations route through the actor mailbox with
    /// the active `TurnId` and emit `CandidateChanged` audit events.
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn candidates(&self) -> crate::query::candidates::MemoryCandidateHandle {
        self.candidates.clone()
    }

    /// Screen-image vision summarization handle, bypassing the
    /// turn-execution actor mailbox entirely.
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    pub fn vision(&self) -> crate::vision::VisionHandle {
        self.vision.clone()
    }

    /// Tool registry operations handle (list / search / call / invalidate).
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    /// Unlike [`EneHandle::sessions`] / [`EneHandle::candidates`] /
    /// [`EneHandle::vision`] it routes through the actor mailbox — tool
    /// calls and searches are admission-capped there (Stage 8) and the
    /// registry is actor-owned state; see [`crate::tools`].
    pub fn tools(&self) -> crate::tools::ToolHandle {
        self.tools.clone()
    }

    /// Workspace document index operations handle (sync / cancel / status /
    /// search).
    ///
    /// Cheap to call repeatedly; the returned handle is a small `Clone`.
    /// Routes through the actor mailbox: sync is single-flight and
    /// cancellable there.
    pub fn workspace(&self) -> crate::workspace::WorkspaceHandle {
        crate::workspace::WorkspaceHandle::new(Arc::clone(&self.cmd_tx))
    }

    /// Hot-swap the character card (CLI `/card`).
    ///
    /// Replaces the loaded card on the actor's session, resets the proactive
    /// scheduler, and keeps the shared card-name slot used by
    /// [`EneHandle::candidates`] in sync.
    pub async fn set_character(
        &self,
        card: ene_config::CharacterCardV3,
    ) -> Result<(), EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SetCharacter {
                card: Box::new(card),
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Open the session with the greeting at `index` (`0` = `first_mes`,
    /// `i+1` = `alternate_greetings[i]`).
    ///
    /// Applies the greeting as the character's opening message and records
    /// the active greeting index for `@@is_greeting` lorebook gating. Rejected
    /// when the card has no greeting at `index` or the session already has
    /// messages. Returns the applied (CBS-expanded) greeting text.
    ///
    /// # Errors
    ///
    /// Returns [`PublicApiError::ActorDead`] if the actor is no longer
    /// running, or [`PublicApiError::Invalid`] for the validation failures
    /// above.
    pub async fn set_greeting(&self, index: u32) -> Result<String, PublicApiError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SetGreeting { index, reply: tx })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)?
    }

    /// Manually trigger a compression-only pass over the current conversation.
    ///
    /// Compression trims history into a stored scene summary but does **not**
    /// start a new session: the session id is unchanged, and the returned
    /// [`ene_mind::CompressionResult`] reflects those compression-only
    /// semantics (no `SplitReason`, no `new_session_id`).
    ///
    /// # Errors
    ///
    /// Returns [`EneRuntimeError::Mind`] wrapping
    /// [`ene_mind::EneSessionError::SplitNotNeeded`] when there is no
    /// history to compress or memory is disabled.
    pub async fn compress_context(&self) -> Result<ene_mind::CompressionResult, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::CompressContext { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Start a turn. Returns [`RunError::Busy`] if a turn is already in flight
    /// — never silently aborts the active turn.
    ///
    /// A dead actor is reported as [`RunError::ActorDead`] rather than
    /// [`RunError::Busy`]: the closed-channel check runs *before* the
    /// single-flight gate, so a `run()` racing the actor's shutdown never
    /// sees a gate the dying actor still holds and reports `Busy` for an
    /// actor that is gone.
    #[must_use = "the returned TurnId is needed for cancellation"]
    pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError> {
        // Dead-actor check first: the command channel closes when the actor
        // task exits, so a closed channel is the authoritative "actor is
        // gone" signal and must win over a stale single-flight gate.
        if self.cmd_tx.is_closed() {
            return Err(RunError::ActorDead);
        }
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

    /// The id of the turn currently in flight, or `None` when the actor is
    /// idle.
    ///
    /// This is the lightweight resynchronization query consumers use after a
    /// broadcast lag. It reads the single-flight [`TurnGate`] directly — one
    /// `parking_lot` lock, no mailbox round-trip — so it returns promptly even
    /// while a heavy turn is streaming, and it never queues behind the actor
    /// loop the way [`crate::diagnostics::EneDiagnostics::get_snapshot`] does.
    ///
    /// ## Recovery procedure after a chat-bus lag
    ///
    /// [`EneEventReceiver::recv`] returns
    /// [`tokio::sync::broadcast::error::RecvError::Lagged`] once the consumer
    /// has fallen behind the chat ring buffer. Every event skipped by the lag
    /// — including the active turn's [`EneEvent::Terminal`] — must be treated
    /// as lost, so the consumer's streamed view of the in-flight turn is no
    /// longer trustworthy. The uniform recovery, shared by every consumer
    /// (CLI and desktop), is:
    ///
    /// 1. Call `active_turn()`. `Some(turn)` means the turn is *still* running
    ///    on the actor (its `Terminal` was among the dropped events); `None`
    ///    means it already finished and the gate was released.
    /// 2. When `Some(turn)`, call [`EneHandle::cancel`] with it so the actor
    ///    emits a fresh `Terminal` and releases the single-flight gate.
    ///    Without this the gate stays held and the next
    ///    [`EneHandle::run`] fails with [`RunError::Busy`].
    /// 3. Tear down any local per-turn UI state (stream buffers, audio
    ///    `is_final` tracking) as though the turn had ended.
    ///
    /// A lifecycle-bus lag ([`LifecycleReceiver`]) needs no cancellation:
    /// lifecycle notifications are turn-independent, so the consumer simply
    /// re-derives the affected state — e.g. re-query
    /// [`EneHandle::candidates`] after missing a `PendingCandidateAvailable`.
    #[must_use]
    pub fn active_turn(&self) -> Option<TurnId> {
        self.turn_gate.active.lock().clone()
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

    /// Gracefully shuts down the plugin host if it is still alive.
    ///
    /// Called from [`EneHandle::shutdown`] after the actor drains. The
    /// [`HandleShutdownGuard`] performs an equivalent shutdown on last-handle
    /// drop; both paths `.take()` from the same `Arc<Mutex<Option<…>>>`, so
    /// only the first caller performs the shutdown — subsequent calls are
    /// no-ops. Also aborts the plugin health → diagnostics bridge task so it
    /// does not outlive the host whose events it forwards.
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

        let mut acceptor = self.host_service_handle.lock().await;
        if let Some(handle) = acceptor.take() {
            handle.abort();
        }
    }

    /// Send a permission decision for a pending destructive operation.
    pub fn decide_permission(
        &self,
        request_id: impl Into<RequestId>,
        decision: PermissionDecision,
    ) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::PermissionDecision {
                request_id: request_id.into(),
                decision,
            })
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Create a persistent schedule.
    ///
    /// Validation (kind fields, timezone, cron expression, future one-shot
    /// `start_at`) happens before anything is persisted; the returned
    /// schedule carries its first computed `next_run_at`.
    pub async fn add_schedule(
        &self,
        new: ene_core::NewSchedule,
    ) -> Result<ene_core::Schedule, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::AddSchedule { new, reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// List all persistent schedules, ordered by name.
    pub async fn list_schedules(&self) -> Result<Vec<ene_core::Schedule>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListSchedules { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Build a chat provider for the configured chat task.
    ///
    /// Routes through the live provider host, so the returned provider
    /// matches what a real turn would use. Used by CLI commands that need a
    /// provider outside a turn (e.g. the memory-write retry drain).
    pub async fn create_chat_provider(
        &self,
    ) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::CreateChatProvider { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await
            .map_err(|_| EneRuntimeError::ChannelClosed)?
            .map_err(|message| {
                EneRuntimeError::Ai(ene_ai::AiError::Llm(ene_ai::LlmProviderError::Provider(
                    message,
                )))
            })
    }

    /// List recent run history for one schedule, newest first.
    pub async fn list_schedule_runs(
        &self,
        schedule_id: i64,
        limit: u64,
    ) -> Result<Vec<ene_core::ScheduleRun>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListScheduleRuns {
                schedule_id,
                limit,
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Delete a schedule and its run history.
    pub async fn delete_schedule(&self, schedule_id: i64) -> Result<bool, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::DeleteSchedule {
                schedule_id,
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Pause (`false`) or resume (`true`) a schedule.
    pub async fn set_schedule_enabled(
        &self,
        schedule_id: i64,
        enabled: bool,
    ) -> Result<bool, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SetScheduleEnabled {
                schedule_id,
                enabled,
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// List all session-wide permission grants.
    pub async fn list_permissions(
        &self,
    ) -> Result<Vec<crate::streaming::PermissionScope>, PublicApiError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListPermissions { reply })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Revoke a single session-wide permission grant by id.
    pub async fn revoke_permission(&self, id: u64) -> Result<bool, PublicApiError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::RevokePermission { id, reply })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Revoke all session-wide permission grants.
    pub async fn reset_all_permissions(&self) -> Result<usize, PublicApiError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ResetAllPermissions { reply })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Undo the most recent reversible tool operation.
    pub async fn undo(&self) -> Result<crate::undo::UndoReport, PublicApiError> {
        let (reply, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::Undo { reply })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Send a user-input response for a pending interactive tool.
    pub fn submit_user_input(
        &self,
        request_id: impl Into<RequestId>,
        response: UserInputResponse,
    ) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::UserInputResponse {
                request_id: request_id.into(),
                response,
            })
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Push a privacy-safe proactive observation from the host.
    pub fn update_proactive_observation(
        &self,
        observation: ene_mind::ProactiveObservation,
    ) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::UpdateProactiveObservation { observation })
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Report a detected system-audio beat pulse for broadcast.
    ///
    /// The actor relays the pulse as [`EneEvent::BeatPulse`] on the chat bus.
    /// The caller is expected to send normalized values (tempo in a plausible
    /// range, intensity in `[0, 1]`); the runtime forwards them verbatim.
    pub fn report_beat_pulse(&self, bpm: f32, intensity: f32) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::BeatPulse { bpm, intensity })
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Hot-update proactive policy in the running actor.
    ///
    /// Prefer [`Self::update_feature_settings`] when emotion / store / tools
    /// also change — this path only patches `mind.proactive` and does not
    /// reload the local decision model.
    pub fn update_proactive_settings(
        &self,
        mind: ene_mind::ProactiveConfig,
    ) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::UpdateProactiveSettings { mind })
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Hot-update Features-tab sections (mind / store / tools / RAG).
    ///
    /// Does not tear down local GGUF handles. Tool process registry is rebuilt
    /// when the tools section changes.
    pub fn update_feature_settings(
        &self,
        settings: FeatureSettingsUpdate,
    ) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::UpdateFeatureSettings {
                settings: Box::new(settings),
            })
            .map_err(|_| PublicApiError::ActorDead)
    }
}

/// Maps a [`PluginHealthEvent`] to a [`DiagnosticEvent::ToolHealth`]
/// with a stable English status contract.
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

/// Resolve a TTS provider from the host registry.
///
/// Falls back to the in-process `ene-voice` Kokoro factory when the host
/// cannot serve the kind (host down or the plugin absent), preserving the
/// pre-plugin behavior. The fallback disappears once `ene-voice` is
/// pluginized and this registry is removed.
async fn resolve_tts_provider(
    host: &dyn ene_ai::ProviderHost,
    config: &EneConfig,
    kind: &str,
) -> Result<Box<dyn ene_ai::TtsProvider>, String> {
    match host.create_tts_provider(kind, config).await {
        Ok(provider) => Ok(provider),
        Err(host_error) => ene_ai::AudioProviderRegistry::create_tts_provider(kind, config)
            .map_err(|legacy_error| format!("host: {host_error}; legacy: {legacy_error}")),
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

    /// The bootstrap-time bridge maps [`DisabledReason`] to its detail string
    /// via an exhaustive match, keeping the `"disabled"` status contract
    /// stable. Kept in lockstep with the actor-side mapper's test.
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
    /// requirements named in the detail. Kept in lockstep with the actor-side
    /// mapper's test.
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

    #[tokio::test]
    async fn broadcast_channels_emit_lag_on_overflow() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");

        let mut event_rx = handle.subscribe();
        let mut diag_rx = handle.diagnostics().subscribe();

        for i in 0..1025 {
            drop(handle.event_tx.send(EneEvent::TextDelta {
                turn: TurnId::new(),
                origin: crate::types::TurnOrigin::User,
                delta: format!("delta {i}"),
            }));
        }

        let res = event_rx.recv().await;
        assert!(
            matches!(
                res,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
            ),
            "Expected RecvError::Lagged for event channel overflow, got {res:?}"
        );

        let mut saw_lagged = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let crate::diagnostics::DiagnosticEvent::Lagged { channel, .. } = ev
                && channel == "events"
            {
                saw_lagged = true;
            }
        }
        assert!(
            saw_lagged,
            "expected DiagnosticEvent::Lagged after event lag"
        );

        let mut diag_rx = handle.diagnostics().subscribe();

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
    async fn set_greeting_applies_message_and_blocks_mid_session() {
        let mut card = test_card();
        card.data.name = "Ene".into();
        card.data.first_mes = "Hello {{user}}!".into();
        card.data.alternate_greetings = vec!["Hey there.".into()];
        let handle = EneHandle::open(test_config_memory_off(), card)
            .await
            .expect("open initializes handle");

        let text = handle
            .set_greeting(0)
            .await
            .expect("first_mes greeting applies");
        assert_eq!(text, "Hello User!");
        let history = handle.history().await.expect("history");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, ene_ai::Role::Assistant);
        assert_eq!(history[0].content, "Hello User!");

        let err = handle
            .set_greeting(1)
            .await
            .expect_err("mid-session greeting rejected");
        assert!(matches!(err, PublicApiError::Invalid { .. }));
    }

    #[tokio::test]
    async fn set_greeting_validates_index_and_card() {
        let mut card = test_card();
        card.data.name = "Ene".into();
        card.data.alternate_greetings = vec!["Hey there.".into()];
        let handle = EneHandle::open(test_config_memory_off(), card)
            .await
            .expect("open initializes handle");

        let text = handle
            .set_greeting(1)
            .await
            .expect("alternate greeting applies");
        assert_eq!(text, "Hey there.");

        let err = handle
            .set_greeting(2)
            .await
            .expect_err("out-of-range greeting rejected");
        assert!(matches!(err, PublicApiError::Invalid { .. }));

        let empty = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");
        let err = empty
            .set_greeting(0)
            .await
            .expect_err("card without greetings rejected");
        assert!(matches!(err, PublicApiError::Invalid { .. }));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn set_greeting_rejected_while_turn_in_flight() {
        let (config, _hanging) = test_config_hanging_provider().await;
        let mut card = test_card();
        card.data.name = "Ene".into();
        card.data.first_mes = "Hello!".into();
        let handle = EneHandle::open_with_provider_host(config, card, hanging_provider_host())
            .await
            .expect("open initializes handle");

        // The hanging provider keeps this turn in flight; the actor processes
        // `Run` before the queued `SetGreeting`, so the in-flight guard fires.
        let turn = handle.run("hello").expect("run claims the gate");
        let err = handle
            .set_greeting(0)
            .await
            .expect_err("in-flight greeting rejected");
        assert!(matches!(err, PublicApiError::Invalid { .. }));

        handle.turn_gate.end();
        drop(handle.cancel(&turn));
        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    /// Config whose chat provider points at a local listener that completes
    /// the TCP handshake (via the kernel backlog) but never writes a response,
    /// so any started turn hangs on the per-request timeout instead of
    /// completing fast and releasing the single-flight gate on its own. The
    /// caller must hold the returned listener for as long as the turn should
    /// stay in flight.
    /// Stub host standing in for the plugin registry in tests: serves the
    /// hanging LLM factory under its custom kind and the hanging embedding
    /// factory under `openai`.
    struct StubProviderHost {
        llm: std::collections::HashMap<String, Arc<dyn ene_ai::LlmProviderFactory>>,
        embedding: std::collections::HashMap<String, Arc<dyn ene_ai::EmbeddingProviderFactory>>,
    }

    #[async_trait::async_trait]
    impl ene_ai::ProviderHost for StubProviderHost {
        async fn create_llm_provider(
            &self,
            kind: &str,
            config: &ene_config::EneConfig,
            task: &ene_ai::config::TaskRef,
        ) -> Result<Box<dyn ene_ai::LlmProvider>, ene_ai::LlmProviderError> {
            self.llm
                .get(kind)
                .ok_or_else(|| {
                    ene_ai::LlmProviderError::Provider(format!(
                        "No LlmProviderFactory registered for provider kind: '{kind}'"
                    ))
                })?
                .create_provider(config, task)
        }

        async fn create_embedding_provider(
            &self,
            kind: &str,
            config: &ene_config::EneConfig,
        ) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
            self.embedding
                .get(kind)
                .ok_or_else(|| {
                    ene_ai::EmbeddingError::Init(format!(
                        "No embedding provider factory registered for provider kind: '{kind}'"
                    ))
                })?
                .create_embedding_provider(config)
        }

        async fn create_tts_provider(
            &self,
            _kind: &str,
            _config: &ene_config::EneConfig,
        ) -> Result<Box<dyn ene_ai::TtsProvider>, ene_ai::AudioProviderError> {
            Err(ene_ai::AudioProviderError::Provider(
                "stub host serves no TTS providers".to_string(),
            ))
        }
    }

    fn hanging_provider_host() -> Arc<dyn ene_ai::ProviderHost> {
        Arc::new(StubProviderHost {
            llm: std::collections::HashMap::from([(
                HangingLlmFactory::KIND.to_string(),
                Arc::new(HangingLlmFactory) as Arc<dyn ene_ai::LlmProviderFactory>,
            )]),
            embedding: std::collections::HashMap::from([(
                ene_ai::config::OPENAI_PROVIDER_KIND.to_string(),
                Arc::new(HangingEmbeddingFactory) as Arc<dyn ene_ai::EmbeddingProviderFactory>,
            )]),
        })
    }

    async fn test_config_hanging_provider() -> (EneConfig, tokio::net::TcpListener) {
        let hanging = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local hanging listener");
        let addr = hanging.local_addr().expect("listener has a local address");
        let mut config = test_config_memory_off();
        // The pre-turn phase fails fast without a store, so give the turn a
        // functional in-memory store: it keeps every DB operation off disk
        // while letting the turn actually reach the hanging provider.
        let store = ene_store::StoreConfig {
            enabled: true,
            in_memory: true,
            ..Default::default()
        };
        config.set_section(&store).expect("store config merges");
        let mut ai = ene_ai::AiConfig::default();
        if let Some(provider) = ai.providers.get_mut("default") {
            provider.base_url = format!("http://{addr}");
            // The OpenAI backend ships as a plugin binary and the plugin host
            // is disabled in tests, so route the chat turn to an in-process
            // factory that never completes.
            provider.kind = HangingLlmFactory::KIND.to_string();
        }
        // Embeddings resolve through the same registry indirection as chat
        // providers; a never-arriving vector keeps the turn in flight at the
        // pre-turn embedding step, deterministically, for both tests below.
        ai.providers.insert(
            "embed-stub".to_string(),
            ene_ai::config::AiProviderDef {
                kind: ene_ai::config::OPENAI_PROVIDER_KIND.to_string(),
                base_url: format!("http://{addr}"),
                api_key: ene_ai::config::ApiKeyConfig {
                    source: "inline".to_string(),
                    inline: "sk-test".to_string(),
                    env: String::new(),
                },
                ..ene_ai::config::AiProviderDef::default()
            },
        );
        ai.tasks.embedding = ene_ai::config::TaskRef {
            provider: "embed-stub".to_string(),
            model: Some("test-embedding-model".to_string()),
            dimensions: Some(8),
            ..ene_ai::config::TaskRef::default()
        };
        drop(config.set_section(&ai));
        (config, hanging)
    }

    /// A chat provider that never completes, standing in for a plugin-backed
    /// provider in tests that need the single-flight gate to stay held while
    /// the turn is in flight.
    struct HangingLlmProvider;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for HangingLlmProvider {
        fn name(&self) -> &str {
            HangingLlmFactory::KIND
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::message::LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn tokio_stream::Stream<
                            Item = Result<
                                ene_ai::message::LlmResponseChunk,
                                ene_ai::error::LlmProviderError,
                            >,
                        > + Send,
                >,
            >,
            ene_ai::error::LlmProviderError,
        > {
            std::future::pending().await
        }

        async fn chat_completion(
            &self,
            _messages: &[ene_ai::message::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<ene_ai::message::LlmCompletion, ene_ai::error::LlmProviderError> {
            std::future::pending().await
        }
    }

    /// Factory producing [`HangingLlmProvider`] under the kind the hanging
    /// provider test config selects.
    struct HangingLlmFactory;

    impl HangingLlmFactory {
        const KIND: &'static str = "test-hanging";
    }

    impl ene_ai::LlmProviderFactory for HangingLlmFactory {
        fn provider_name(&self) -> &str {
            Self::KIND
        }

        fn create_provider(
            &self,
            _config: &EneConfig,
            _task: &ene_ai::config::TaskRef,
        ) -> Result<Box<dyn ene_ai::LlmProvider>, ene_ai::error::LlmProviderError> {
            Ok(Box::new(HangingLlmProvider))
        }
    }

    /// An embedding provider whose vectors never arrive, keeping the turn in
    /// flight at the pre-turn embedding step (see [`HangingLlmFactory`]).
    struct HangingEmbeddingProvider;

    #[async_trait::async_trait]
    impl ene_ai::EmbeddingProvider for HangingEmbeddingProvider {
        async fn embed_batch(
            &self,
            _items: &[(&str, ene_ai::EmbeddingKind)],
        ) -> Result<Vec<Vec<f32>>, ene_ai::EmbeddingError> {
            std::future::pending().await
        }

        fn dimensions(&self) -> usize {
            8
        }

        fn model_name(&self) -> &'static str {
            "test-hanging"
        }
    }

    /// Factory producing [`HangingEmbeddingProvider`] under the `openai` kind
    /// the test config's embedding task selects.
    struct HangingEmbeddingFactory;

    impl ene_ai::EmbeddingProviderFactory for HangingEmbeddingFactory {
        fn provider_kind(&self) -> &str {
            ene_ai::config::OPENAI_PROVIDER_KIND
        }

        fn create_embedding_provider(
            &self,
            _config: &EneConfig,
        ) -> Result<std::sync::Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError> {
            Ok(std::sync::Arc::new(HangingEmbeddingProvider))
        }
    }

    /// `active_turn()` is a lock-only read of the shared single-flight gate:
    /// `None` while idle, `Some(id)` once `run` has claimed the gate,
    /// and `None` again after the gate is released. No mailbox round-trip is
    /// involved, which is what makes it usable as a lag-recovery probe.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn active_turn_reflects_single_flight_gate() {
        let (config, _hanging) = test_config_hanging_provider().await;
        let handle =
            EneHandle::open_with_provider_host(config, test_card(), hanging_provider_host())
                .await
                .expect("open initializes handle");

        assert!(
            handle.active_turn().is_none(),
            "no turn is in flight before run()"
        );

        // `run` claims the gate synchronously before the command is even
        // delivered to the actor, and the hanging provider keeps the turn in
        // flight, so the probe deterministically observes `Some` here.
        let turn = handle.run("hello").expect("run claims the gate");
        assert_eq!(
            handle.active_turn().as_ref(),
            Some(&turn),
            "active_turn() must report the in-flight turn id"
        );

        handle.turn_gate.end();
        assert!(
            handle.active_turn().is_none(),
            "active_turn() must clear once the gate is released"
        );

        drop(handle.cancel(&turn));
        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    /// End-to-end lag recovery: after the chat bus overflows and the
    /// consumer observes `RecvError::Lagged`, the documented resync path —
    /// `active_turn()` then `cancel()` — restores a consistent view: the
    /// in-flight turn is still discoverable through the lightweight probe
    /// (not the stale, gap-ridden stream), cancellation succeeds, and the
    /// actor emits a fresh `Terminal` so the gate is released. Also asserts
    /// the lag surfaced exactly one `DiagnosticEvent::Lagged` and no
    /// `ResyncNeeded`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resync_after_chat_bus_lag_restores_consistent_view() {
        // Keep the turn in flight deterministically (see the helper's doc):
        // the hanging provider means the turn cannot complete and release the
        // gate on its own while the lag recovery runs.
        let (config, _hanging) = test_config_hanging_provider().await;
        let handle =
            EneHandle::open_with_provider_host(config, test_card(), hanging_provider_host())
                .await
                .expect("open initializes handle");

        let mut event_rx = handle.subscribe();
        let mut diag_rx = handle.diagnostics().subscribe();

        let turn = handle.run("hello").expect("run claims the gate");

        for i in 0..1025 {
            drop(handle.event_tx.send(EneEvent::TextDelta {
                turn: TurnId::new(),
                origin: crate::types::TurnOrigin::User,
                delta: format!("delta {i}"),
            }));
        }

        let res = event_rx.recv().await;
        assert!(
            matches!(
                res,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))
            ),
            "expected RecvError::Lagged after overflowing the chat bus, got {res:?}"
        );

        // Resync step 1: the in-flight turn is still discoverable through the
        // mailbox-free probe even though its events were dropped.
        assert_eq!(
            handle.active_turn().as_ref(),
            Some(&turn),
            "active_turn() must still report the in-flight turn after a lag"
        );

        // Resync step 2: cancelling it succeeds and makes the actor emit a
        // fresh Terminal, releasing the single-flight gate.
        handle
            .cancel(&turn)
            .expect("cancel must succeed for the active turn");

        let mut saw_terminal = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !saw_terminal && tokio::time::Instant::now() < deadline {
            match event_rx.recv().await {
                Ok(EneEvent::Terminal { turn: t, .. }) if t == turn => saw_terminal = true,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
        assert!(
            saw_terminal,
            "the actor must emit a fresh Terminal after the lag-recovery cancel"
        );

        // The actor emits `Terminal` a few instructions before releasing the
        // gate, so poll briefly for the release rather than asserting it the
        // instant the Terminal is observed.
        let gate_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while handle.active_turn().is_some() && tokio::time::Instant::now() < gate_deadline {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            handle.active_turn().is_none(),
            "the gate must be released once the recovered turn terminates"
        );

        // The lag surfaced as exactly the observability twin of the recv
        // error: a Lagged diagnostic for the events channel, and no
        // ResyncNeeded.
        let mut saw_lagged = false;
        while let Ok(ev) = diag_rx.try_recv() {
            match ev {
                crate::diagnostics::DiagnosticEvent::Lagged { channel, .. }
                    if channel == "events" =>
                {
                    saw_lagged = true;
                }
                other => {
                    let debug = format!("{other:?}");
                    assert!(
                        !debug.contains("ResyncNeeded"),
                        "ResyncNeeded must no longer be emitted, got {debug}"
                    );
                }
            }
        }
        assert!(
            saw_lagged,
            "expected DiagnosticEvent::Lagged for the events channel"
        );

        drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    }

    /// Regression test: the `Drop` for `EneHandle` must not gate shutdown
    /// on `Arc::strong_count(&cmd_tx) == 1`, which never holds because a
    /// single handle holds four independent `Arc` clones of `cmd_tx`
    /// (self, diagnostics, vision, tools). The [`HandleShutdownGuard`]
    /// is shared via one `Arc` across all handle clones, so its `Drop` must
    /// fire exactly once — when the *last* clone is released. Dropping an
    /// intermediate clone must leave the actor alive; dropping the last
    /// clone must make it exit.
    #[tokio::test]
    async fn drop_last_handle_clone_shuts_down_actor_exactly_once() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");

        // Probe sender held outside the handle: the guard does not depend
        // on `cmd_tx`'s strong count, so an extra sender clone must not
        // defer shutdown.
        let probe = Arc::clone(&handle.cmd_tx);

        let clone = handle.clone();
        drop(handle);

        // Dropping one of two clones must not shut the actor down: a mailbox
        // round-trip still completes.
        let scopes = clone
            .list_permissions()
            .await
            .expect("actor must stay alive while one handle clone remains");
        assert!(
            scopes.is_empty(),
            "no permissions granted in a fresh runtime"
        );

        // Dropping the last clone fires `HandleShutdownGuard::drop`, which
        // sends `EneCommand::Shutdown`. The actor processes it, exits its
        // command loop, and drops its receiver, after which probe sends fail.
        drop(clone);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let (reply, _rx) = oneshot::channel();
            if probe.send(EneCommand::ListPermissions { reply }).is_err() {
                break; // receiver dropped -> actor exited
            }
            assert!(
                std::time::Instant::now() < deadline,
                "actor still running after the last handle clone dropped; \
                 HandleShutdownGuard did not send Shutdown"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Regression test: an explicit [`EneHandle::shutdown`] before
    /// drop must leave the guard's `Drop` a silent no-op — the command
    /// channel is closed and the shutdown mutexes already emptied, so no
    /// double shutdown or panic occurs.
    #[tokio::test]
    async fn explicit_shutdown_before_drop_is_idempotent() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");

        handle
            .shutdown(std::time::Duration::from_secs(5))
            .await
            .expect("explicit shutdown drains the actor");

        // Guard `Drop` runs here: every cleanup action is a no-op because
        // the actor already consumed its receiver and `shutdown_plugin_host`
        // already `.take()`-ed the host and bridge. Passes if it neither
        // panics nor hangs.
        drop(handle);
    }

    /// Regression test: explicit shutdown on one clone while a
    /// sibling clone is still live. The shared guard fires only when the
    /// last clone releases it — by which point every resource it touches
    /// has already been consumed by the explicit shutdown.
    #[tokio::test]
    async fn explicit_shutdown_with_live_clone_then_drop_last() {
        let handle = EneHandle::open(test_config_memory_off(), test_card())
            .await
            .expect("open initializes handle");
        let clone = handle.clone();

        handle
            .shutdown(std::time::Duration::from_secs(5))
            .await
            .expect("explicit shutdown drains the actor");

        // `clone` still references the guard, so it has not dropped yet.
        // These drops bring the guard's refcount to zero and run its `Drop`
        // against already-emptied state. Passes if it neither panics nor
        // hangs.
        drop(clone);
        drop(handle);
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

        let mut saw_panic = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let crate::diagnostics::DiagnosticEvent::ActorPanic { component, .. } = ev {
                assert_eq!(component, "test-component");
                saw_panic = true;
            }
        }
        assert!(saw_panic, "expected ActorPanic diagnostic event");
    }

    /// Regression test: `catch_unwind`-based isolation in
    /// `run_command_isolated` must keep the actor usable after a command
    /// panics. This drives a full mailbox round trip through a
    /// live actor (unlike `isolate_panic_contains_panic_and_emits_diagnostic`,
    /// which calls the bare `isolate_panic` helper directly) and asserts all
    /// four properties:
    ///   (a) the panic is caught, not propagated out of the actor task,
    ///   (b) the actor survives and keeps processing commands afterward,
    ///   (c) a `DiagnosticEvent::ActorPanic { component: "command", .. }` fires,
    ///   (d) `pending_permissions`, `permission_scopes`, and `undo_stack` —
    ///       the three shared-state fields a panicking command can mutate — are
    ///       left in a consistent, still-usable state: the mutations recorded
    ///       just before the panic are fully present, not torn or lost, and the
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
        // return `PublicApiError::ActorDead` / a dropped reply channel. It doesn't.
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
        // locked, or the sender dropped, this would fail (ActorDead) or
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

    /// Regression test: a read-only session query must not queue
    /// behind an in-flight `Run` turn.
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

    /// Regression test: after the actor has shut down, a
    /// subsequent `run()` must report [`RunError::ActorDead`], never
    /// [`RunError::Busy`].
    ///
    /// A `run()` that consults the single-flight gate *before* the command
    /// channel could surface `Busy` from a gate the dying actor still holds
    /// even though the actor is gone. The code checks the closed channel
    /// first and releases the gate on `Shutdown`, so a dead actor is
    /// reported as dead.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_after_shutdown_reports_actor_dead_not_busy() {
        let config = test_config_memory_off();
        let handle = EneHandle::open(config, test_card())
            .await
            .expect("open initializes handle");

        // Drain the actor and wait for its task to fully exit; `shutdown`
        // awaits the actor's `JoinHandle`, so on return the command channel
        // is closed.
        handle
            .shutdown(std::time::Duration::from_secs(2))
            .await
            .expect("shutdown completes");

        assert!(
            matches!(handle.run("hello"), Err(RunError::ActorDead)),
            "run() after shutdown must report ActorDead, not a stale Busy"
        );
    }

    // ── Stage 8: bounded actor task admission ──

    /// A `ToolRegistry` with no tools, whose `call_tool` always succeeds.
    /// Good enough for admission-cap tests below, which only care about how
    /// many times a `JoinSet` was asked to admit a task, not about real tool
    /// behavior. `pub(super)` so `actor::tests` can reuse it.
    pub(super) struct EmptyRegistry;

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
    /// mock tool call actually completes. `pub(super)` so `actor::tests` can
    /// reuse it.
    pub(super) fn build_bare_actor(
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        task_caps: &crate::task_config::ToolRuntimeConfig,
    ) -> (actor::TurnActor, broadcast::Receiver<DiagnosticEvent>) {
        let (actor, diag_rx, _lifecycle_rx) = build_bare_actor_with_lifecycle(registry, task_caps);
        (actor, diag_rx)
    }

    /// Like [`build_bare_actor`] but also hands back the lifecycle-bus
    /// receiver, so admission tests can assert on
    /// [`LifecycleEvent::PendingCandidateAvailable`]. The registry is
    /// taken explicitly (same signature as [`build_bare_actor`]); callers
    /// that do not care about tool behavior pass `Arc::new(EmptyRegistry)`.
    fn build_bare_actor_with_lifecycle(
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        task_caps: &crate::task_config::ToolRuntimeConfig,
    ) -> (
        actor::TurnActor,
        broadcast::Receiver<DiagnosticEvent>,
        broadcast::Receiver<LifecycleEvent>,
    ) {
        let (actor, diag_rx, lifecycle_rx, _gate) = build_bare_actor_with_gate(registry, task_caps);
        (actor, diag_rx, lifecycle_rx)
    }

    /// Like [`build_bare_actor_with_lifecycle`] but also hands back the shared
    /// [`TurnGate`], so tests can assert on single-flight gate state — e.g.
    /// that `Shutdown` releases it. `pub(super)` so `actor::tests` can
    /// reuse it.
    pub(super) fn build_bare_actor_with_gate(
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        task_caps: &crate::task_config::ToolRuntimeConfig,
    ) -> (
        actor::TurnActor,
        broadcast::Receiver<DiagnosticEvent>,
        broadcast::Receiver<LifecycleEvent>,
        Arc<TurnGate>,
    ) {
        let (actor, diag_rx, lifecycle_rx, gate, _shared) = build_bare_actor_with_session_and_gate(
            registry,
            task_caps,
            ConversationSession::new(),
            EneConfig::default(),
        );
        (actor, diag_rx, lifecycle_rx, gate)
    }

    /// Like [`build_bare_actor_with_gate`] but with an injected session and
    /// config, and also hands back the shared [`SharedActorState`] so tests
    /// can assert on the mailbox-free slots. `pub(super)` so
    /// `actor::tests` can reuse it for session-split / config-sync tests.
    pub(super) fn build_bare_actor_with_session_and_gate(
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
        task_caps: &crate::task_config::ToolRuntimeConfig,
        session: ConversationSession,
        mut config: EneConfig,
    ) -> (
        actor::TurnActor,
        broadcast::Receiver<DiagnosticEvent>,
        broadcast::Receiver<LifecycleEvent>,
        Arc<TurnGate>,
        Arc<SharedActorState>,
    ) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, _event_rx) = broadcast::channel(16);
        let (lifecycle_tx, lifecycle_rx) = broadcast::channel(16);
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let (diag_tx, diag_rx) = broadcast::channel(64);
        config
            .set_section(task_caps)
            .expect("set_section(ToolRuntimeConfig) succeeds");
        let health_monitor =
            ene_ai::ProviderHealthMonitor::new(std::time::Duration::from_secs(1), 8);
        let gate = Arc::new(TurnGate::new());
        let shared = Arc::new(SharedActorState::new(&config, &session));
        let actor = actor::TurnActor::new(
            cmd_rx,
            cmd_tx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            diag_tx.clone(),
            diag_tx.subscribe(),
            Arc::clone(&gate),
            config,
            session,
            None,
            registry,
            None,
            None,
            health_monitor,
            None,
            Vec::new(),
            Arc::new(crate::provider_host::LiveProviderCatalog::new(Arc::new(
                tokio::sync::Mutex::new(None),
            ))),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::clone(&shared),
        );
        (actor, diag_rx, lifecycle_rx, gate, shared)
    }

    /// Like [`build_bare_actor_with_session_and_gate`] but with an injected
    /// in-memory store and a chat-event receiver, for scheduler tests that
    /// drive `handle_command` directly (no `run()` loop, no timer task).
    pub(super) fn build_bare_actor_with_store(
        store: Arc<ene_store::MemoryStore>,
        registry: Arc<dyn ene_plugin_host::ToolRegistry>,
    ) -> (
        actor::TurnActor,
        broadcast::Receiver<EneEvent>,
        Arc<TurnGate>,
    ) {
        let task_caps = crate::task_config::ToolRuntimeConfig::default();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = broadcast::channel(64);
        let (lifecycle_tx, _lifecycle_rx) = broadcast::channel(16);
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let (diag_tx, _diag_rx) = broadcast::channel(64);
        let mut config = EneConfig::default();
        config
            .set_section(&task_caps)
            .expect("set_section(ToolRuntimeConfig) succeeds");
        let health_monitor =
            ene_ai::ProviderHealthMonitor::new(std::time::Duration::from_secs(1), 8);
        let gate = Arc::new(TurnGate::new());
        let session = ConversationSession::new();
        let shared = Arc::new(SharedActorState::new(&config, &session));
        let actor = actor::TurnActor::new(
            cmd_rx,
            cmd_tx,
            event_tx,
            lifecycle_tx,
            audio_tx,
            diag_tx.clone(),
            diag_tx.subscribe(),
            Arc::clone(&gate),
            config,
            session,
            Some(store),
            registry,
            None,
            None,
            health_monitor,
            None,
            Vec::new(),
            Arc::new(crate::provider_host::LiveProviderCatalog::new(Arc::new(
                tokio::sync::Mutex::new(None),
            ))),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::clone(&shared),
        );
        (actor, event_rx, gate)
    }

    fn new_test_schedule(name: &str) -> ene_core::NewSchedule {
        ene_core::NewSchedule {
            name: name.to_string(),
            kind: ene_core::ScheduleKind::Cron,
            timezone: "UTC".to_string(),
            cron_expr: Some("0 9 * * *".to_string()),
            interval_secs: None,
            start_at: None,
            action: ene_core::ScheduleAction::Tool {
                name: "test.tool".to_string(),
                arguments: serde_json::Value::Null,
            },
            confirmation: ene_core::ScheduleConfirmation::None,
            max_retries: 0,
            retry_delay_secs: 60,
        }
    }

    async fn add_schedule_via_actor(
        actor: &mut actor::TurnActor,
        new: ene_core::NewSchedule,
    ) -> ene_core::Schedule {
        let (tx, rx) = oneshot::channel();
        assert!(
            actor
                .handle_command(EneCommand::AddSchedule { new, reply: tx })
                .await
        );
        rx.await
            .expect("AddSchedule replies")
            .expect("schedule inserted")
    }

    /// A fire whose `scheduled_at` no longer matches the store's
    /// `next_run_at` is a stale duplicate and must not claim a second run.
    #[tokio::test]
    async fn stale_schedule_fire_is_ignored() {
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, _event_rx, _gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));
        let schedule = add_schedule_via_actor(&mut actor, new_test_schedule("daily")).await;
        let fire = schedule.next_run_at.expect("first fire time");

        assert!(
            actor
                .handle_command(EneCommand::ScheduleFire {
                    schedule_id: schedule.id,
                    scheduled_at: fire + chrono::Duration::hours(1),
                })
                .await
        );
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert!(runs.is_empty(), "stale fire must not create a run row");
    }

    /// A fire processed while the single-flight gate is held is recorded
    /// `skipped_busy` and never starts a turn.
    #[tokio::test]
    async fn busy_fire_is_recorded_skipped_busy() {
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, mut event_rx, gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));
        let schedule = add_schedule_via_actor(&mut actor, new_test_schedule("daily")).await;
        let fire = schedule.next_run_at.expect("first fire time");

        assert!(gate.try_begin(&TurnId::new()), "test holds the gate");
        assert!(
            actor
                .handle_command(EneCommand::ScheduleFire {
                    schedule_id: schedule.id,
                    scheduled_at: fire,
                })
                .await
        );
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert_eq!(runs[0].status, ene_core::ScheduleRunStatus::SkippedBusy);
        assert!(
            event_rx.try_recv().is_err(),
            "a skipped fire must not emit turn events"
        );
    }

    /// A host-reported beat pulse is relayed verbatim to chat-bus
    /// subscribers, independent of any active turn.
    #[tokio::test]
    async fn beat_pulse_command_broadcasts_event() {
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, mut event_rx, _gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));

        assert!(
            actor
                .handle_command(EneCommand::BeatPulse {
                    bpm: 123.0,
                    intensity: 0.75,
                })
                .await
        );

        match event_rx.recv().await {
            Ok(EneEvent::BeatPulse { bpm, intensity }) => {
                assert!((bpm - 123.0).abs() < f32::EPSILON);
                assert!((intensity - 0.75).abs() < f32::EPSILON);
            }
            other => panic!("expected BeatPulse on the chat bus, got {other:?}"),
        }
    }

    /// A fire beyond the late-execution grace window is recorded
    /// `skipped_late` and jumps to the next occurrence without executing.
    #[tokio::test]
    async fn late_fire_is_recorded_skipped_late() {
        use sea_orm::{ConnectionTrait, Statement};
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, mut event_rx, _gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));
        let schedule = add_schedule_via_actor(&mut actor, new_test_schedule("daily")).await;
        let _fire = schedule.next_run_at.expect("first fire time");
        // The actor's late check compares against the real wall clock, so
        // the backdated occurrence must be built from `Utc::now()`, not the
        // schedule's (future) first fire time.
        let missed = chrono::Utc::now() - chrono::Duration::hours(2);
        store
            .connection()
            .execute_raw(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Sqlite,
                "UPDATE schedules SET next_run_at = ? WHERE id = ?",
                [
                    sea_orm::Value::ChronoDateTimeUtc(Some(missed)),
                    schedule.id.into(),
                ],
            ))
            .await
            .expect("backdate next_run_at");

        assert!(
            actor
                .handle_command(EneCommand::ScheduleFire {
                    schedule_id: schedule.id,
                    scheduled_at: missed,
                })
                .await
        );
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert_eq!(runs[0].status, ene_core::ScheduleRunStatus::SkippedLate);
        assert!(event_rx.try_recv().is_err(), "late fire must not execute");
    }

    /// A denied confirmation records `denied` and never starts the action.
    #[tokio::test]
    async fn denied_confirmation_records_denied_run() {
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, mut event_rx, _gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));
        let mut new = new_test_schedule("gated");
        new.confirmation = ene_core::ScheduleConfirmation::Confirm;
        let schedule = add_schedule_via_actor(&mut actor, new).await;
        let fire = schedule.next_run_at.expect("first fire time");

        assert!(
            actor
                .handle_command(EneCommand::ScheduleFire {
                    schedule_id: schedule.id,
                    scheduled_at: fire,
                })
                .await
        );
        let request_id = match event_rx.recv().await.expect("confirmation event") {
            EneEvent::PermissionRequired {
                origin: crate::types::TurnOrigin::Scheduled,
                request_id,
                ..
            } => request_id,
            other => panic!("expected scheduled PermissionRequired, got {other:?}"),
        };
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert_eq!(
            runs[0].status,
            ene_core::ScheduleRunStatus::AwaitingApproval
        );
        assert!(
            actor
                .handle_command(EneCommand::PermissionDecision {
                    request_id,
                    decision: PermissionDecision::Deny,
                })
                .await
        );
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert_eq!(runs[0].status, ene_core::ScheduleRunStatus::Denied);
    }

    /// An approved confirmation moves the run out of `awaiting_approval`
    /// before execution starts, so a stale timeout can never record it
    /// `timed_out` mid-run.
    #[tokio::test]
    async fn approved_confirmation_marks_run_running() {
        let store = Arc::new(
            ene_store::MemoryStore::open_in_memory(4)
                .await
                .expect("in-memory store"),
        );
        let (mut actor, mut event_rx, _gate) =
            build_bare_actor_with_store(store.clone(), Arc::new(EmptyRegistry));
        let mut new = new_test_schedule("gated");
        new.confirmation = ene_core::ScheduleConfirmation::Confirm;
        let schedule = add_schedule_via_actor(&mut actor, new).await;
        let fire = schedule.next_run_at.expect("first fire time");

        assert!(
            actor
                .handle_command(EneCommand::ScheduleFire {
                    schedule_id: schedule.id,
                    scheduled_at: fire,
                })
                .await
        );
        let request_id = match event_rx.recv().await.expect("confirmation event") {
            EneEvent::PermissionRequired {
                origin: crate::types::TurnOrigin::Scheduled,
                request_id,
                ..
            } => request_id,
            other => panic!("expected scheduled PermissionRequired, got {other:?}"),
        };
        assert!(
            actor
                .handle_command(EneCommand::PermissionDecision {
                    request_id,
                    decision: PermissionDecision::AllowOnce,
                })
                .await
        );
        let runs = store.list_runs(schedule.id, 10).await.expect("list runs");
        assert_eq!(
            runs[0].status,
            ene_core::ScheduleRunStatus::Running,
            "approved run must leave awaiting_approval before executing"
        );
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

        // Placeholder tasks must stay in-flight (not complete) so they are
        // still counted when `admit_task` reaps before its capacity check;
        // an `async {}` body would finish instantly and be reaped
        // away, so the cap would never be reached.
        assert!(actor::admit_task(&mut set, 2, "TestSet", None, &diag_tx));
        set.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        assert!(actor::admit_task(&mut set, 2, "TestSet", None, &diag_tx));
        set.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });
        assert!(!actor::admit_task(
            &mut set,
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

    /// Regression test: `admit_task` must reap
    /// completed-but-unjoined tasks *before* checking the cap, so a burst
    /// that finished during the actor's `select!` block does not cause a
    /// spurious `EneRuntimeError::Busy`.
    ///
    /// Also guards the converse: a task that is *still running* must keep
    /// counting against the cap, so a genuine `Busy` is preserved.
    #[tokio::test]
    async fn admit_task_reaps_finished_tasks_and_preserves_real_busy() {
        let (diag_tx, _diag_rx) = broadcast::channel(16);
        let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_task = std::sync::Arc::clone(&release);
        assert!(actor::admit_task(&mut set, 1, "TestSet", None, &diag_tx));
        set.spawn(async move {
            release_task.notified().await;
        });

        // Still running → genuinely at capacity → rejected (real Busy).
        assert!(
            !actor::admit_task(&mut set, 1, "TestSet", None, &diag_tx),
            "an in-flight task must still count against the cap"
        );

        // Release the task and let it run to completion. It is now
        // completed but not yet joined, so `set.len()` still counts it.
        release.notify_one();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // Without reaping first, a completed-but-unjoined task keeps
        // `len() == 1` and admission would reject spuriously; reaping frees
        // the slot so admission succeeds.
        assert!(
            actor::admit_task(&mut set, 1, "TestSet", None, &diag_tx),
            "a completed-but-unjoined task must not count against the cap"
        );
        assert_eq!(set.len(), 0, "the finished task should have been reaped");
    }

    /// Regression test: a memory-writer handle rejected by the
    /// Stage 8 cap must still have its outcome consumed, so
    /// [`LifecycleEvent::PendingCandidateAvailable`] reaches the lifecycle
    /// bus instead of being silently dropped.
    ///
    /// `memory_writer_cap` is set to 0 so *every* drained handle is
    /// rejected purely by the cap (no placeholder task needed to fill the
    /// `JoinSet`).
    #[tokio::test]
    async fn rejected_memory_writer_still_emits_pending_candidate() {
        let task_caps = crate::task_config::ToolRuntimeConfig {
            memory_writer_cap: 0,
            ..Default::default()
        };
        let (mut actor, mut diag_rx, mut lifecycle_rx) =
            build_bare_actor_with_lifecycle(Arc::new(EmptyRegistry), &task_caps);

        let handle = tokio::spawn(async {
            ene_mind::MemoryWriteOutcome::Ok {
                deferred_candidates: 3,
            }
        });
        actor.inject_and_drain_memory_writer(handle);

        let ev = tokio::time::timeout(std::time::Duration::from_secs(2), lifecycle_rx.recv())
            .await
            .expect("PendingCandidateAvailable must arrive even when admission is rejected")
            .expect("lifecycle channel still open");
        assert!(
            matches!(ev, LifecycleEvent::PendingCandidateAvailable { count: 3 }),
            "expected PendingCandidateAvailable{{count:3}}, got {ev:?}"
        );

        let mut saw_rejected = false;
        while let Ok(ev) = diag_rx.try_recv() {
            if let DiagnosticEvent::TaskRejected { component, cap, .. } = ev
                && component == "MemoryWriter"
                && cap == 0
            {
                saw_rejected = true;
            }
        }
        assert!(
            saw_rejected,
            "expected a TaskRejected diagnostic for MemoryWriter"
        );
    }

    /// Companion to the test above for the failure path: a rejected
    /// memory-writer whose write failed must still emit
    /// [`DiagnosticEvent::MemoryWrite`]. With no store configured the
    /// pending/permanent counts are `None` and the status is `"failed"`.
    #[tokio::test]
    async fn rejected_memory_writer_still_emits_failure_diagnostic() {
        let task_caps = crate::task_config::ToolRuntimeConfig {
            memory_writer_cap: 0,
            ..Default::default()
        };
        let (mut actor, mut diag_rx, _lifecycle_rx) =
            build_bare_actor_with_lifecycle(Arc::new(EmptyRegistry), &task_caps);

        let handle = tokio::spawn(async {
            ene_mind::MemoryWriteOutcome::Failed {
                message: "boom".to_string(),
                pending_id: None,
                permanent: false,
                character_id: "char".to_string(),
            }
        });
        actor.inject_and_drain_memory_writer(handle);

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut saw_memory_write = false;
        while tokio::time::Instant::now() < deadline && !saw_memory_write {
            match tokio::time::timeout(std::time::Duration::from_millis(50), diag_rx.recv()).await {
                Ok(Ok(DiagnosticEvent::MemoryWrite {
                    character_id,
                    status,
                    message,
                    pending_id,
                    pending_count,
                    permanent_count,
                })) => {
                    assert_eq!(character_id, "char");
                    assert_eq!(status, "failed");
                    assert_eq!(message, "boom");
                    assert!(pending_id.is_none());
                    assert!(pending_count.is_none());
                    assert!(permanent_count.is_none());
                    saw_memory_write = true;
                }
                Ok(Err(e)) => panic!("diagnostic channel closed early: {e}"),
                // An unrelated diagnostic (e.g. TaskRejected) or a poll
                // timeout: keep scanning until the deadline.
                Ok(Ok(_)) | Err(_) => {}
            }
        }
        assert!(
            saw_memory_write,
            "expected a MemoryWrite diagnostic even when admission is rejected"
        );
    }

    /// Mixed admission scenario: with `memory_writer_cap` set to 1, both
    /// drained handles are supervised in `memory_writer_tasks` (the second
    /// waits on the semaphore). Both must still emit
    /// [`LifecycleEvent::PendingCandidateAvailable`].
    #[tokio::test]
    async fn mixed_admitted_and_queued_memory_writers_both_emit_pending_candidate() {
        let task_caps = crate::task_config::ToolRuntimeConfig {
            memory_writer_cap: 1,
            ..Default::default()
        };
        let (mut actor, mut diag_rx, mut lifecycle_rx) =
            build_bare_actor_with_lifecycle(Arc::new(EmptyRegistry), &task_caps);

        // Each handle reports a distinct deferred-candidate count so the two
        // events can be told apart. With semaphore concurrency 1, the second
        // consumer waits in the JoinSet until the first finishes.
        for count in [3_usize, 7_usize] {
            let handle = tokio::spawn(async move {
                ene_mind::MemoryWriteOutcome::Ok {
                    deferred_candidates: count,
                }
            });
            actor.inject_and_drain_memory_writer(handle);
        }

        let mut counts = Vec::new();
        while counts.len() < 2 {
            let ev = tokio::time::timeout(std::time::Duration::from_secs(2), lifecycle_rx.recv())
                .await
                .expect(
                    "PendingCandidateAvailable must arrive for both the running and the queued handle",
                )
                .expect("lifecycle channel still open");
            match ev {
                LifecycleEvent::PendingCandidateAvailable { count } => counts.push(count),
                other => panic!("expected PendingCandidateAvailable, got {other:?}"),
            }
        }
        counts.sort_unstable();
        assert_eq!(
            counts,
            vec![3, 7],
            "both the running and the queued memory writer must emit their pending-candidate event"
        );

        while let Ok(ev) = diag_rx.try_recv() {
            if let DiagnosticEvent::TaskRejected { component, .. } = ev {
                panic!(
                    "queued memory writers must not be hard-dropped, got TaskRejected for {component}"
                );
            }
        }
    }

    /// When the supervised waiter queue hits the memory-writer hard limit
    /// (`memory_writer_cap * 4`), further handles are hard-dropped:
    /// `TaskRejected` fires and no lifecycle event is emitted for the dropped
    /// handle.
    #[tokio::test]
    async fn memory_writer_hard_drop_emits_rejected_without_lifecycle() {
        let task_caps = crate::task_config::ToolRuntimeConfig {
            memory_writer_cap: 1,
            ..Default::default()
        };
        let (mut actor, mut diag_rx, mut lifecycle_rx) =
            build_bare_actor_with_lifecycle(Arc::new(EmptyRegistry), &task_caps);

        // Hold the semaphore with hanging outcome consumers so the JoinSet
        // fills to the hard limit (cap * 4 = 4) without reaping.
        let mut blockers = Vec::new();
        for _ in 0..4 {
            let (block_tx, block_rx) = oneshot::channel::<()>();
            blockers.push(block_tx);
            let handle = tokio::spawn(async move {
                block_rx.await.ok();
                ene_mind::MemoryWriteOutcome::Ok {
                    deferred_candidates: 1,
                }
            });
            actor.inject_and_drain_memory_writer(handle);
        }

        let dropped = tokio::spawn(async {
            ene_mind::MemoryWriteOutcome::Ok {
                deferred_candidates: 99,
            }
        });
        actor.inject_and_drain_memory_writer(dropped);

        let mut saw_rejected = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline && !saw_rejected {
            match tokio::time::timeout(std::time::Duration::from_millis(50), diag_rx.recv()).await {
                Ok(Ok(DiagnosticEvent::TaskRejected { component, cap, .. }))
                    if component == "MemoryWriter" && cap == 4 =>
                {
                    saw_rejected = true;
                }
                Ok(Err(e)) => panic!("diagnostic channel closed early: {e}"),
                Ok(Ok(_)) | Err(_) => {}
            }
        }
        assert!(
            saw_rejected,
            "expected TaskRejected when the memory-writer hard queue limit is hit"
        );

        // The hard-dropped handle must not produce a lifecycle event. Drain
        // briefly to ensure 99 never arrives; hanging consumers are still
        // blocked so only a spurious drop would emit.
        let spurious =
            tokio::time::timeout(std::time::Duration::from_millis(100), lifecycle_rx.recv()).await;
        if let Ok(Ok(LifecycleEvent::PendingCandidateAvailable { count: 99 })) = spurious {
            panic!("hard-dropped memory writer must not emit PendingCandidateAvailable");
        }

        // Unblock hanging consumers so the JoinSet can finish cleanly.
        for tx in blockers {
            tx.send(()).ok();
        }
    }

    /// Regression test on the `Err(e)` arm of
    /// [`actor::consume_memory_write_outcome`]: a memory-writer `JoinHandle`
    /// whose task panicked resolves to `Err`, and the cap-0 reject+consume
    /// path must still surface the failure as a
    /// [`DiagnosticEvent::MemoryWrite`] — with an empty `character_id`, a
    /// `"failed"` status, and no pending/permanent counts (no store
    /// configured).
    #[tokio::test]
    async fn panicked_memory_writer_still_emits_failure_diagnostic() {
        let task_caps = crate::task_config::ToolRuntimeConfig {
            memory_writer_cap: 0,
            ..Default::default()
        };
        let (mut actor, mut diag_rx, _lifecycle_rx) =
            build_bare_actor_with_lifecycle(Arc::new(EmptyRegistry), &task_caps);

        // A panicked task makes the JoinHandle resolve `Err(JoinError)`,
        // exercising the `Err(e)` arm of `consume_memory_write_outcome`.
        let handle: tokio::task::JoinHandle<ene_mind::MemoryWriteOutcome> =
            tokio::spawn(async { panic!("boom") });
        actor.inject_and_drain_memory_writer(handle);

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut saw_memory_write = false;
        while tokio::time::Instant::now() < deadline && !saw_memory_write {
            match tokio::time::timeout(std::time::Duration::from_millis(50), diag_rx.recv()).await {
                Ok(Ok(DiagnosticEvent::MemoryWrite {
                    character_id,
                    status,
                    message,
                    pending_id,
                    pending_count,
                    permanent_count,
                })) => {
                    assert_eq!(character_id, "");
                    assert_eq!(status, "failed");
                    assert!(
                        message.starts_with("memory writer task panicked:"),
                        "expected a panic-derived message, got {message:?}"
                    );
                    assert!(pending_id.is_none());
                    assert!(pending_count.is_none());
                    assert!(permanent_count.is_none());
                    saw_memory_write = true;
                }
                Ok(Err(e)) => panic!("diagnostic channel closed early: {e}"),
                // An unrelated diagnostic (e.g. TaskRejected) or a poll
                // timeout: keep scanning until the deadline.
                Ok(Ok(_)) | Err(_) => {}
            }
        }
        assert!(
            saw_memory_write,
            "expected a MemoryWrite diagnostic even when the writer task panicked"
        );
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

    /// Records the per-call context of every `call_tool` invocation so
    /// tests can pin the host's scoping contract.
    struct RecordingContextRegistry {
        contexts: Arc<std::sync::Mutex<Vec<Option<ene_plugin_proto::CallContext>>>>,
    }

    #[async_trait::async_trait]
    impl ene_plugin_host::ToolRegistry for RecordingContextRegistry {
        fn list_tools(&self) -> Vec<ene_plugin_proto::ToolSpec> {
            Vec::new()
        }

        async fn call_tool(
            &self,
            _name: &str,
            _arguments: &str,
            context: Option<&ene_plugin_proto::CallContext>,
        ) -> Result<ene_plugin_proto::ToolResult, ene_plugin_host::PluginHostError> {
            self.contexts.lock().unwrap().push(context.cloned());
            Ok(ene_plugin_proto::ToolResult::text("done"))
        }
    }

    /// Direct `CallTool` commands (turn: `None`, e.g. `ene-cli tool call`)
    /// must still receive a per-call context: a unique synthetic turn keeps
    /// plugin approval scopes from leaking between calls.
    #[tokio::test]
    async fn direct_call_tool_always_receives_a_fresh_synthetic_context() {
        let contexts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let registry: Arc<dyn ene_plugin_host::ToolRegistry> = Arc::new(RecordingContextRegistry {
            contexts: contexts.clone(),
        });
        let task_caps = crate::task_config::ToolRuntimeConfig {
            call_tool_cap: 4,
            ..Default::default()
        };
        let (mut actor, _diag_rx) = build_bare_actor(registry, &task_caps);

        let explicit_turn = crate::types::TurnId::new();
        for turn in [None, Some(explicit_turn.clone()), None] {
            let (tx, rx) = oneshot::channel();
            assert!(
                actor
                    .handle_command(EneCommand::CallTool {
                        name: "anything".into(),
                        arguments: "{}".into(),
                        turn,
                        reply: tx,
                    })
                    .await
            );
            rx.await
                .expect("reply channel not dropped")
                .expect("direct call must succeed");
        }

        let recorded = contexts.lock().unwrap();
        assert_eq!(recorded.len(), 3);
        let first = recorded[0]
            .as_ref()
            .expect("direct call must receive a context");
        let second = recorded[1]
            .as_ref()
            .expect("turned call must receive a context");
        let third = recorded[2]
            .as_ref()
            .expect("direct call must receive a context");
        assert!(
            first.turn_id.starts_with("direct:"),
            "synthetic turn id expected, got {}",
            first.turn_id
        );
        assert_ne!(
            first.turn_id, third.turn_id,
            "each direct call must get a fresh scope"
        );
        assert_eq!(second.turn_id, explicit_turn.to_string());
        assert_eq!(
            first.conversation_id, third.conversation_id,
            "direct calls stay in the session conversation"
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

    // ── No head-of-line blocking in the command loop ──

    /// Regression test: a heavy command whose work has been moved
    /// off the actor loop into `bg_command_tasks` must not block subsequent
    /// commands. A handler awaiting heavy I/O *inline* (GGUF model load,
    /// plugin-host restart) would stall the
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
