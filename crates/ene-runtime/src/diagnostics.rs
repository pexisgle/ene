//! Diagnostics facade for opt-in pipeline and memory APIs.
//!
//! Chat-critical events stay on [`crate::EneEvent`]. This facade is strictly
//! *observability*: pipeline phases/metrics, provider health/fallback
//! history, actor snapshots, and memory/journal inspection. Control surface —
//! character-card swap ([`crate::EneHandle::set_character`]), manual context
//! compression ([`crate::EneHandle::compress_context`]), and tool
//! operations ([`crate::EneHandle::tools`]) — moved back onto
//! [`crate::EneHandle`] itself.
//!
//! The memory surface (`MemoryHandle`) is the one mutation-capable part of
//! this facade (pin/status/restore/forget, commitment lifecycle, pending-write
//! inspection/drain, store backup/integrity). It deliberately does **not**
//! expose the raw `MemoryStore`: the former `store()` accessor was removed
//! so consumers must go through the facade's own methods, which carry
//! the same `require_store()` errors as the rest of the surface.

use crate::error::EneRuntimeError;
use crate::handle::{EneCommand, EneStateSnapshot};
use crate::public_api::PublicApiError;
use crate::types::TurnId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot};

/// A handle for querying **and mutating** the memory subsystem.
///
/// Wraps the memory store and embedding provider, exposing the operations
/// needed by downstream consumers (CLI commands, desktop bridge, etc.).
/// Despite living on the diagnostics facade it is not read-only:
/// pin/status/restore/forget mutations, commitment lifecycle
/// (complete/cancel), pending-write inspection and drain, and store
/// backup/integrity diagnostics are part of its surface, which is why the
/// type is named [`MemoryHandle`] rather than a "query" handle.
///
/// The raw `MemoryStore` is intentionally not exposed (`store()` was removed);
/// every operation routes through the facade's own methods and
/// reports `EneRuntimeError::Memory` (connection error when memory is
/// disabled) exactly like the rest of the surface.
#[derive(Clone)]
pub struct MemoryHandle {
    pub(crate) store: Option<Arc<ene_store::MemoryStore>>,
    pub(crate) embedder: Option<Arc<dyn ene_ai::EmbeddingProvider>>,
    pub(crate) mind_memory: ene_mind::MindMemoryConfig,
    /// Shared L1 recall cache invalidated by every memory mutation here.
    pub(crate) recall_cache: Option<Arc<ene_mind::MemoryRecallCache>>,
}

impl std::fmt::Debug for MemoryHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryHandle")
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl MemoryHandle {
    pub(crate) fn new(
        store: Option<Arc<ene_store::MemoryStore>>,
        embedder: Option<Arc<dyn ene_ai::EmbeddingProvider>>,
        mind_memory: ene_mind::MindMemoryConfig,
        recall_cache: Option<Arc<ene_mind::MemoryRecallCache>>,
    ) -> Self {
        Self {
            store,
            embedder,
            mind_memory,
            recall_cache,
        }
    }

    /// Drop the L1 cache after a mutation that only identifies the row by id.
    ///
    /// Coarse but rare (user-driven UI actions), so the correctness win of
    /// never serving stale gather results outweighs the cache rebuild.
    fn invalidate_recall_cache(&self) {
        if let Some(cache) = &self.recall_cache {
            cache.invalidate_all();
        }
    }

    /// Whether memory is enabled and both store and embedder are available.
    pub fn is_enabled(&self) -> bool {
        self.store.is_some() && self.embedder.is_some()
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
            .get_typed_memories_by_character(character_id, kind, None, None, limit, 0)
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
        let result = store
            .pin_typed_memory(id, pinned)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
    }

    /// Transition typed memory lifecycle status.
    pub async fn set_memory_status(
        &self,
        id: i64,
        status: ene_store::MemoryStatus,
    ) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        let result = store
            .set_memory_status(id, status)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
    }

    /// User-driven restore to active status (journal/CLI UX).
    pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        let result = store
            .user_restore_typed_memory(id)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
    }

    /// User-driven forget (`Active` → `UserDeleted`).
    pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        let result = store
            .user_forget_typed_memory(id)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
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
    ///
    /// # Actor consistency
    ///
    /// [`ene_mind::commitments::CommitmentLedger`] holds no in-memory cache
    /// and the actor re-reads active commitments from the store at prompt
    /// injection, so this direct store write cannot desync the actor.
    pub async fn complete_commitment(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        let result = store
            .complete_commitment(id)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
    }

    /// Mark a commitment as cancelled.
    ///
    /// Same actor-consistency guarantee as [`Self::complete_commitment`]:
    /// the ledger is stateless, so a direct store write is harmless.
    pub async fn cancel_commitment(&self, id: i64) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        let result = store
            .cancel_commitment(id)
            .await
            .map_err(EneRuntimeError::Memory);
        if result.as_ref().is_ok_and(|changed| *changed) {
            self.invalidate_recall_cache();
        }
        result
    }

    /// Count pending (retryable) and permanent failed memory writes.
    pub async fn count_pending_memory_writes(
        &self,
        character_id: &str,
    ) -> Result<(usize, usize), EneRuntimeError> {
        let store = self.require_store()?;
        store
            .count_pending_memory_writes(character_id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// List pending / permanent memory-write rows for a character.
    pub async fn list_pending_memory_writes(
        &self,
        character_id: &str,
        limit: usize,
    ) -> Result<Vec<ene_core::PendingMemoryWrite>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .list_pending_memory_writes(character_id, limit)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Force pending rows for a character to be due immediately.
    ///
    /// Used by `/memory retry` so the operator can drain the queue without
    /// waiting for exponential backoff.
    pub async fn schedule_pending_memory_writes_now(
        &self,
        character_id: &str,
    ) -> Result<usize, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .schedule_pending_memory_writes_now(character_id)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Drain due pending memory writes for a character using the mind engine.
    ///
    /// The caller must schedule the writes as due first (via
    /// [`Self::schedule_pending_memory_writes_now`]); this method only drains
    /// the now-due batch, running [`ene_mind::CognitionEngine`]'s drain pass
    /// with this handle's embedder. Errors are surfaced by
    /// [`ene_mind::CognitionEngine::drain_pending_memory_writes`] silently
    /// (it is a best-effort drain), so this only fails when the store is
    /// unavailable.
    pub async fn drain_pending_memory_writes(
        &self,
        config: &ene_mind::MindConfig,
        llm: &dyn ene_ai::LlmProvider,
        limit: usize,
    ) -> Result<(), EneRuntimeError> {
        let store = self.require_store()?;
        let embedder = self.embedder.as_ref().map(std::convert::AsRef::as_ref);
        ene_mind::CognitionEngine::drain_pending_memory_writes(
            store.as_ref(),
            config,
            llm,
            embedder,
            limit,
            self.recall_cache.as_deref(),
        )
        .await;
        Ok(())
    }

    /// Create a timestamped file backup of this store's database.
    ///
    /// Returns an error when the store is in-memory (no path).
    pub async fn backup(&self) -> Result<std::path::PathBuf, EneRuntimeError> {
        let store = self.require_store()?;
        store.backup().await.map_err(EneRuntimeError::Memory)
    }

    /// On-disk path of the store's database, when opened from a file.
    ///
    /// Exposed (rather than the store itself) so backup/restore commands can
    /// locate the database file without reaching for the store handle.
    #[must_use]
    pub fn path(&self) -> Option<&std::path::Path> {
        self.store.as_ref().and_then(|s| s.path())
    }

    /// Run `PRAGMA integrity_check` on the open connection.
    pub async fn check_integrity(&self) -> Result<(), EneRuntimeError> {
        let store = self.require_store()?;
        store
            .check_integrity()
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

#[cfg(test)]
mod tests {
    use super::*;

    type StreamResult = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<ene_ai::LlmResponseChunk, ene_ai::LlmProviderError>,
                > + Send,
        >,
    >;

    /// Minimal `LlmProvider` double whose completions always fail, used to
    /// exercise facade methods that take an `&dyn LlmProvider` (the pending
    /// write drain) without touching a real backend.
    struct FailingLlm;

    #[async_trait::async_trait]
    impl ene_ai::LlmProvider for FailingLlm {
        fn name(&self) -> &'static str {
            "failing-test-llm"
        }

        async fn create_chat_stream(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _tools: &[ene_plugin_proto::ToolSpec],
        ) -> Result<StreamResult, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "test double: streaming not implemented".to_string(),
            ))
        }

        async fn chat_completion(
            &self,
            _messages: &[ene_ai::LlmMessage],
            _json_schema: Option<serde_json::Value>,
        ) -> Result<ene_ai::LlmCompletion, ene_ai::LlmProviderError> {
            Err(ene_ai::LlmProviderError::Provider(
                "test double: forced extraction failure".to_string(),
            ))
        }
    }

    async fn test_handle() -> MemoryHandle {
        let store = ene_store::MemoryStore::open_in_memory(4).await.unwrap();
        MemoryHandle::new(
            Some(std::sync::Arc::new(store)),
            None,
            ene_mind::MindMemoryConfig::default(),
            None,
        )
    }

    fn storeless_handle() -> MemoryHandle {
        MemoryHandle::new(None, None, ene_mind::MindMemoryConfig::default(), None)
    }

    /// Assert a facade call failed with the disabled-store connection error.
    fn assert_connection_error<T: std::fmt::Debug>(result: &Result<T, EneRuntimeError>) {
        assert!(
            matches!(
                result,
                Err(EneRuntimeError::Memory(
                    ene_store::EneMemoryError::MemoryStoreConnectionError(_)
                ))
            ),
            "expected connection error, got {result:?}"
        );
    }

    async fn insert_commitment(handle: &MemoryHandle, title: &str) -> i64 {
        let store = handle.store.as_ref().unwrap();
        store
            .insert_commitment(&ene_store::NewCommitment {
                character_id: "ene".into(),
                user_id: "user1".into(),
                title: title.into(),
                description: format!("{title} description"),
                status: ene_store::CommitmentStatus::Active,
                due_at: None,
                due_label: None,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn complete_and_cancel_commitment_round_trip() {
        let handle = test_handle().await;

        let done_id = insert_commitment(&handle, "finish review").await;
        assert!(
            handle.complete_commitment(done_id).await.unwrap(),
            "active commitment completes"
        );
        assert!(
            !handle.complete_commitment(done_id).await.unwrap(),
            "already-done commitment returns false"
        );

        let cancelled_id = insert_commitment(&handle, "cancel meeting").await;
        assert!(
            handle.cancel_commitment(cancelled_id).await.unwrap(),
            "active commitment cancels"
        );
        assert!(
            !handle.cancel_commitment(cancelled_id).await.unwrap(),
            "already-cancelled commitment returns false"
        );
    }

    #[tokio::test]
    async fn count_pending_memory_writes_round_trip() {
        let handle = test_handle().await;

        let store = handle.store.as_ref().unwrap();
        store
            .enqueue_pending_memory_write(
                "ene",
                "user1",
                r#"{"character_id":"ene","user_id":"user1"}"#,
                "boom",
            )
            .await
            .unwrap();

        let (pending, permanent) = handle.count_pending_memory_writes("ene").await.unwrap();
        assert_eq!((pending, permanent), (1, 0));

        let rows = handle.list_pending_memory_writes("ene", 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows.first().map(|r| r.status),
            Some(ene_core::PendingMemoryWriteStatus::Pending)
        );
    }

    #[tokio::test]
    async fn count_pending_memory_writes_reports_permanent_failures() {
        let handle = test_handle().await;
        let store = handle.store.as_ref().unwrap();
        let id = store
            .enqueue_pending_memory_write("ene", "user1", "{}", "boom")
            .await
            .unwrap();

        // Exhaust the retry budget so the row transitions to permanent.
        for _ in 0..(ene_store::MemoryStore::PENDING_MEMORY_WRITE_MAX_ATTEMPTS - 1) {
            store
                .fail_pending_memory_write(id, "retry failed")
                .await
                .unwrap();
        }

        let (pending, permanent) = handle.count_pending_memory_writes("ene").await.unwrap();
        assert_eq!((pending, permanent), (0, 1), "exhausted row is permanent");
    }

    #[tokio::test]
    async fn schedule_and_drain_pending_memory_writes_round_trip() {
        let handle = test_handle().await;
        let store = handle.store.as_ref().unwrap();
        store
            .enqueue_pending_memory_write(
                "ene",
                "user1",
                r#"{"character_id":"ene","user_id":"user1"}"#,
                "boom",
            )
            .await
            .unwrap();

        // Freshly enqueued rows wait out their backoff; scheduling forces them
        // due immediately so a drain can pick them up.
        let scheduled = handle
            .schedule_pending_memory_writes_now("ene")
            .await
            .unwrap();
        assert_eq!(scheduled, 1, "one pending row forced due");

        // The drain is best-effort: the facade returns Ok even when the queued
        // payload cannot be replayed, and the row is retained with its retry
        // count bumped rather than dropped.
        handle
            .drain_pending_memory_writes(&ene_mind::MindConfig::default(), &FailingLlm, 10)
            .await
            .unwrap();

        let (pending, permanent) = handle.count_pending_memory_writes("ene").await.unwrap();
        assert_eq!(
            (pending, permanent),
            (1, 0),
            "row stays pending after failed replay"
        );

        let rows = handle.list_pending_memory_writes("ene", 10).await.unwrap();
        assert_eq!(
            rows.first().map(|r| r.attempts),
            Some(2),
            "drain re-failed the row and bumped its retry count"
        );
    }

    #[tokio::test]
    async fn in_memory_store_path_integrity_and_backup() {
        let handle = test_handle().await;

        assert_eq!(handle.path(), None, "in-memory store has no file path");
        handle.check_integrity().await.unwrap();

        let err = handle.backup().await.unwrap_err();
        assert!(
            matches!(
                err,
                EneRuntimeError::Memory(ene_store::EneMemoryError::BackupError(_))
            ),
            "in-memory stores cannot be backed up to a file, got {err:?}"
        );
    }

    #[tokio::test]
    async fn storeless_handle_reports_connection_error_across_surface() {
        let handle = storeless_handle();

        assert_eq!(handle.path(), None, "store-less handle has no db path");
        assert_connection_error(&handle.complete_commitment(1).await);
        assert_connection_error(&handle.cancel_commitment(1).await);
        assert_connection_error(&handle.backup().await);
        assert_connection_error(&handle.check_integrity().await);
        assert_connection_error(&handle.schedule_pending_memory_writes_now("ene").await);
        assert_connection_error(&handle.count_pending_memory_writes("ene").await);
        assert_connection_error(&handle.list_pending_memory_writes("ene", 10).await);
        assert_connection_error(
            &handle
                .drain_pending_memory_writes(&ene_mind::MindConfig::default(), &FailingLlm, 10)
                .await,
        );
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
///
/// Observability-only surface: memory/journal inspection, provider health,
/// diagnostic-event subscription, and actor snapshots. Control operations
/// (character swap, compression, tools) live on [`crate::EneHandle`].
pub struct EneDiagnostics {
    pub(crate) cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    pub(crate) diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub(crate) memory: MemoryHandle,
    /// Provider health monitor for failover diagnostics.
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
    /// Memory / journal query-and-mutation surface.
    pub const fn memory(&self) -> &MemoryHandle {
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

    /// Probe every chat failover candidate through the provider host.
    ///
    /// Returns one health report per probed candidate (using the monitor's
    /// TTL cache for recent results) and records the outcomes into the
    /// shared monitor. Runs in a background task inside the actor, so slow
    /// probes cannot stall the actor loop.
    pub async fn probe_chat_candidates(&self) -> Vec<ene_ai::ProviderHealthReport> {
        let (tx, rx) = oneshot::channel();
        if self
            .cmd_tx
            .send(EneCommand::ProbeChatCandidates { reply: tx })
            .is_err()
        {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
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
}

pub(crate) fn emit_diag(diag_tx: &broadcast::Sender<DiagnosticEvent>, event: DiagnosticEvent) {
    drop(diag_tx.send(event));
}
