//! Diagnostics facade for opt-in pipeline and memory APIs.
//!
//! Chat-critical events stay on [`crate::EneEvent`]. Pipeline phases/metrics
//! and memory/journal/tool inspection live here.

use crate::error::EneRuntimeError;
use crate::handle::{EneCommand, EneStateSnapshot};
use crate::public_api::PublicApiError;
use crate::types::TurnId;
use ene_mind::SplitResult;
use ene_plugin_proto::ToolSpec;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// A read-only handle for querying the memory subsystem.
///
/// Wraps the memory store and embedding provider, exposing only
/// the operations needed by downstream consumers (CLI commands, etc.).
#[derive(Clone)]
pub struct MemoryQueryHandle {
    pub(crate) store: Option<Arc<ene_store::MemoryStore>>,
    pub(crate) embedder: Option<Arc<dyn ene_ai::EmbeddingProvider>>,
    pub(crate) mind_memory: ene_mind::MindMemoryConfig,
}

impl std::fmt::Debug for MemoryQueryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryQueryHandle")
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl MemoryQueryHandle {
    pub(crate) fn new(
        store: Option<Arc<ene_store::MemoryStore>>,
        embedder: Option<Arc<dyn ene_ai::EmbeddingProvider>>,
        mind_memory: ene_mind::MindMemoryConfig,
    ) -> Self {
        Self {
            store,
            embedder,
            mind_memory,
        }
    }

    /// Whether memory is enabled and both store and embedder are available.
    pub fn is_enabled(&self) -> bool {
        self.store.is_some() && self.embedder.is_some()
    }

    /// Returns the backing memory store when memory is configured.
    pub const fn store(&self) -> Option<&Arc<ene_store::MemoryStore>> {
        self.store.as_ref()
    }

    /// Returns the embedding provider when memory is configured.
    pub fn embedder(&self) -> Option<&Arc<dyn ene_ai::EmbeddingProvider>> {
        self.embedder.as_ref()
    }

    /// Embed a text query for similarity search.
    pub async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EneRuntimeError> {
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            EneRuntimeError::from(ene_ai::EmbeddingError::Init(
                "Embedding provider not available".into(),
            ))
        })?;
        ene_ai::embed_query(embedder.as_ref(), text)
            .await
            .map_err(EneRuntimeError::from)
    }

    /// List typed memories for the current character.
    pub async fn list_typed_memories(
        &self,
        character_id: &str,
        kind: Option<ene_store::MemoryKind>,
        limit: usize,
    ) -> Result<Vec<ene_store::MemoryItem>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .get_typed_memories_by_character(character_id, kind, limit, 0)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// List typed memories for the memory journal with user/scope and status filters.
    pub async fn list_journal_memories(
        &self,
        options: &ene_store::MemoryJournalListOptions<'_>,
    ) -> Result<Vec<ene_store::MemoryItem>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .list_journal_memories(options)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Fetch a typed memory by id.
    pub async fn inspect_typed_memory(
        &self,
        id: i64,
    ) -> Result<Option<ene_store::MemoryItem>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .get_typed_memory(id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Search typed memories using hybrid scoring via [`ene_mind::MemoryJournal`].
    pub async fn search_typed_memories_hybrid(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ene_store::ScoredMemory>, EneRuntimeError> {
        let store = self.require_store()?;
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            EneRuntimeError::from(ene_ai::EmbeddingError::Init(
                "Embedding provider not available".into(),
            ))
        })?;
        ene_mind::MemoryJournal::search(
            store.as_ref(),
            embedder.as_ref(),
            &self.mind_memory,
            character_id,
            user_id,
            query_text,
            limit,
        )
        .await
        .map_err(EneRuntimeError::from)
    }

    /// Search typed memories and attach explainable recall reasons.
    pub async fn search_typed_memories_explained(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<ene_mind::RecalledMemory>, EneRuntimeError> {
        let store = self.require_store()?;
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            EneRuntimeError::from(ene_ai::EmbeddingError::Init(
                "Embedding provider not available".into(),
            ))
        })?;
        ene_mind::MemoryJournal::search_explained(
            store.as_ref(),
            embedder.as_ref(),
            &self.mind_memory,
            character_id,
            user_id,
            query_text,
            limit,
        )
        .await
        .map_err(EneRuntimeError::from)
    }

    /// Update typed memory pinned flag.
    pub async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .pin_typed_memory(id, pinned)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Transition typed memory lifecycle status.
    pub async fn set_memory_status(
        &self,
        id: i64,
        status: ene_store::MemoryStatus,
    ) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .set_memory_status(id, status)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// User-driven restore to active status (journal/CLI UX).
    pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .user_restore_typed_memory(id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// User-driven forget (`Active` → `UserDeleted`).
    pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .user_forget_typed_memory(id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Show current affect state for a character.
    pub async fn show_affect_state(
        &self,
        character_id: &str,
    ) -> Result<ene_store::AffectState, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .get_affect_state(character_id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Reset affect state to neutral baseline.
    pub async fn reset_affect_state(&self, character_id: &str) -> Result<(), EneRuntimeError> {
        let store = self.require_store()?;
        let neutral = ene_store::AffectState::neutral(character_id);
        store
            .upsert_affect_state(&neutral)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// List active commitments for a character/user.
    pub async fn list_active_commitments(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ene_store::Commitment>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .list_active_commitments(character_id, user_id, limit)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Mark a commitment as done.
    pub async fn complete_commitment(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .complete_commitment(id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    fn require_store(&self) -> Result<&std::sync::Arc<ene_store::MemoryStore>, EneRuntimeError> {
        self.store.as_ref().ok_or_else(|| {
            EneRuntimeError::Memory(ene_store::EneMemoryError::MemoryStoreConnectionError(
                "Memory store not available".into(),
            ))
        })
    }
}

/// Opt-in diagnostic events (pipeline detail).
#[derive(Debug, Clone)]
pub enum DiagnosticEvent {
    /// Status update for a long-running pre-generation phase.
    PipelinePhase {
        /// Active turn.
        turn: TurnId,
        /// Short description of the current phase.
        phase: String,
    },
    /// Pre-generation timing summary.
    PipelineMetrics {
        /// Active turn.
        turn: TurnId,
        /// Map of phase names to elapsed time in milliseconds.
        timings: HashMap<String, u64>,
    },
    /// A panic was caught and contained by the actor supervisor.
    ///
    /// The actor loop survives per-command panics; this event surfaces them
    /// for diagnostics instead of crashing the process or losing the panic.
    ActorPanic {
        /// Component that panicked (e.g. `"command"`, `"SearchTools"`).
        component: String,
        /// Best-effort panic message.
        message: String,
    },
    /// A tool health/lifecycle event from the tool host supervisor.
    ///
    /// Emitted when a tool is detected unhealthy (hung or dead), restarted,
    /// recovered, paused by the circuit breaker, or disabled. `status` is a
    /// stable English contract mirroring [`ene_plugin_host::PluginHealthEvent`].
    ToolHealth {
        /// Tool name.
        tool: String,
        /// Stable status code: `unhealthy`, `restarting`, `restarted`,
        /// `recovered`, `circuit_open`, `circuit_closed`, or `disabled`.
        status: String,
        /// Optional human-readable detail (e.g. unhealthy reason).
        detail: Option<String>,
    },
    /// A provider health-check result.
    ///
    /// Emitted after each provider probe so the UI can display the active
    /// provider's connectivity, latency, and last error without polling.
    ProviderHealth {
        /// Provider name (key in `ai.providers`).
        provider: String,
        /// Stable status code: `healthy`, `degraded`, `auth_failed`,
        /// `rate_limited`, `unreachable`, `server_error`, or `unknown`.
        status: String,
        /// Measured round-trip latency in milliseconds (0 if unreachable).
        latency_ms: u64,
        /// Optional human-readable error detail.
        detail: Option<String>,
    },
    /// A provider failover event.
    ///
    /// Emitted when the runtime switches from an unhealthy primary provider
    /// to a fallback so the user is notified that the conversation is
    /// continuing on a different backend.
    ProviderFallback {
        /// Provider that failed.
        from: String,
        /// Provider selected instead.
        to: String,
        /// Reason for the fallback.
        reason: String,
    },
    /// A deferred memory-write failure or permanent queue warning.
    MemoryWrite {
        /// Character scope for the failed write.
        character_id: String,
        /// Stable status: `failed`, `enqueued`, or `permanent`.
        status: String,
        /// Human-readable error detail.
        message: String,
        /// Optional pending-queue row id.
        pending_id: Option<i64>,
        /// Current pending retry count for the character (if known).
        pending_count: Option<u64>,
        /// Current permanent failure count for the character (if known).
        permanent_count: Option<u64>,
    },
    /// A broadcast subscriber lagged and skipped events.
    ///
    /// Consumers must not treat the stream as gap-free after this. The lag is
    /// also surfaced synchronously as the
    /// [`tokio::sync::broadcast::error::RecvError::Lagged`] return value of the
    /// offending `recv`/`try_recv`, which is the signal consumers actually act
    /// on — this diagnostic is the opt-in observability twin of that error.
    ///
    /// The recovery procedure is documented on
    /// [`crate::EneHandle::active_turn`]: for a chat-bus (`events`) lag,
    /// query the in-flight turn and cancel it to release the single-flight
    /// gate; for a lifecycle (`lifecycle`) lag, re-derive the affected state.
    Lagged {
        /// Channel that lagged: `events`, `lifecycle`, or `diagnostics`.
        channel: String,
        /// Number of messages skipped by the broadcast ring.
        skipped: u64,
    },
    /// A background task was refused admission because its actor-owned
    /// `JoinSet` was already at its configured capacity (Stage 8).
    ///
    /// Emitted alongside (not instead of) an [`EneRuntimeError::Busy`] reply
    /// when the rejected command has a reply channel (`CallTool`,
    /// `SearchTools`); for fire-and-forget admission points (deferred tool
    /// pollers, post-turn classifier/memory-writer supervisors) this is the
    /// only signal that the task was dropped rather than tracked.
    TaskRejected {
        /// Which task set rejected the task (e.g. `"CallTool"`,
        /// `"DeferredTool"`, `"Classifier"`, `"MemoryWriter"`,
        /// `"SearchTools"`).
        component: String,
        /// The configured capacity that was already reached.
        cap: usize,
        /// Optional human-readable detail (e.g. tool name / task id).
        detail: Option<String>,
    },
}

/// Extracts a human-readable message from a caught panic payload.
pub(crate) fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Event receiver for [`DiagnosticEvent`].
///
/// On lag, emits [`DiagnosticEvent::Lagged`] onto the same channel before
/// returning the lag error. Diagnostics are observability-only, so a
/// diagnostics-bus lag needs no state recovery beyond noting the gap.
pub struct DiagnosticEventReceiver {
    inner: broadcast::Receiver<DiagnosticEvent>,
    diag_tx: broadcast::Sender<DiagnosticEvent>,
}

impl std::fmt::Debug for DiagnosticEventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticEventReceiver").finish()
    }
}

impl DiagnosticEventReceiver {
    fn note_lag(&self, skipped: u64) {
        drop(self.diag_tx.send(DiagnosticEvent::Lagged {
            channel: "diagnostics".to_string(),
            skipped,
        }));
    }

    /// Non-blocking poll.
    pub fn try_recv(&mut self) -> Result<DiagnosticEvent, broadcast::error::TryRecvError> {
        match self.inner.try_recv() {
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::TryRecvError::Lagged(n))
            }
            other => other,
        }
    }

    /// Async receive.
    pub async fn recv(&mut self) -> Result<DiagnosticEvent, broadcast::error::RecvError> {
        match self.inner.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                self.note_lag(n);
                Err(broadcast::error::RecvError::Lagged(n))
            }
            other => other,
        }
    }
}

/// Concrete diagnostics facade returned by [`crate::EneHandle::diagnostics`].
pub struct EneDiagnostics {
    pub(crate) cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    pub(crate) diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub(crate) memory: MemoryQueryHandle,
    pub(crate) health_monitor: ene_ai::ProviderHealthMonitor,
}

impl std::fmt::Debug for EneDiagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EneDiagnostics")
            .field("memory_enabled", &self.memory.is_enabled())
            .finish_non_exhaustive()
    }
}

impl EneDiagnostics {
    /// Memory / journal query surface.
    pub const fn memory(&self) -> &MemoryQueryHandle {
        &self.memory
    }

    /// Provider health monitor for failover diagnostics.
    pub const fn health_monitor(&self) -> &ene_ai::ProviderHealthMonitor {
        &self.health_monitor
    }

    /// Snapshot of all cached provider health reports.
    pub fn provider_health_reports(&self) -> Vec<ene_ai::ProviderHealthReport> {
        self.health_monitor.all_reports()
    }

    /// Snapshot of recent provider fallback events.
    pub fn provider_fallback_history(&self) -> Vec<ene_ai::FallbackRecord> {
        self.health_monitor.fallback_history()
    }

    /// Subscribe to diagnostic events (pipeline phases/metrics).
    pub fn subscribe(&self) -> DiagnosticEventReceiver {
        DiagnosticEventReceiver {
            inner: self.diag_tx.subscribe(),
            diag_tx: self.diag_tx.clone(),
        }
    }

    /// Request a snapshot of the current actor state.
    pub async fn get_snapshot(&self) -> Result<EneStateSnapshot, PublicApiError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::GetSnapshot { reply: tx })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// List available tools from the registry.
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>, PublicApiError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListTools { reply: tx })
            .map_err(|_| PublicApiError::ActorDead)?;
        rx.await.map_err(|_| PublicApiError::ActorDead)
    }

    /// Search tools in the registry using RAG if available.
    ///
    /// # Errors
    ///
    /// Returns [`EneRuntimeError::Busy`] when the actor's tool-search task
    /// set is at capacity (Stage 8).
    pub async fn search_tools(&self, query: String) -> Result<Vec<ToolSpec>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SearchTools { query, reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Call a tool directly by name with arguments.
    pub async fn call_tool(
        &self,
        name: String,
        arguments: String,
    ) -> Result<String, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::CallTool {
                name,
                arguments,
                turn: None,
                reply: tx,
            })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Manually trigger a session split / compression pass.
    pub async fn manual_split(&self) -> Result<SplitResult, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ManualSplit { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)?
    }

    /// Invalidate the Tool RAG index.
    pub fn invalidate_tool_index(&self) -> Result<(), PublicApiError> {
        self.cmd_tx
            .send(EneCommand::InvalidateToolIndex)
            .map_err(|_| PublicApiError::ActorDead)
    }

    /// Hot-swap the character card (CLI `/card`).
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
}

pub(crate) fn emit_diag(diag_tx: &broadcast::Sender<DiagnosticEvent>, event: DiagnosticEvent) {
    drop(diag_tx.send(event));
}
