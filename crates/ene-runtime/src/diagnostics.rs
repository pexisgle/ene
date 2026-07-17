//! Diagnostics facade for opt-in pipeline and memory APIs (#111).
//!
//! Chat-critical events stay on [`crate::EneEvent`]. Pipeline phases/metrics
//! and memory/journal/tool inspection live here.

use crate::error::EneRuntimeError;
use crate::handle::{ActorDeadError, EneCommand, EneStateSnapshot};
use crate::types::TurnId;
use ene_mind::SplitResult;
use ene_tool_proto::ToolSpec;
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

    /// Count legacy memory rows for a character card.
    pub async fn count_legacy_rows(
        &self,
        card_name: &str,
    ) -> Result<ene_store::LegacyRowCounts, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .count_legacy_rows(card_name)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Migration status for a character card.
    pub async fn migration_status(
        &self,
        card_name: &str,
    ) -> Result<Option<ene_store::MigrationStatus>, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .get_migration_status(card_name)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Run legacy → typed one-shot migration.
    pub async fn migrate_legacy(
        &self,
        card_name: &str,
        user_id: &str,
        dry_run: bool,
    ) -> Result<ene_store::LegacyMigrationReport, EneRuntimeError> {
        let store = self.require_store()?;
        let model = self
            .embedder
            .as_ref()
            .ok_or_else(|| {
                EneRuntimeError::from(ene_ai::EmbeddingError::Init(
                    "Embedding provider not available".into(),
                ))
            })?
            .model_name()
            .to_string();
        let options = ene_store::LegacyMigrationOptions {
            card_name: card_name.to_string(),
            user_id: user_id.to_string(),
            embedding_model: model,
            dry_run,
        };
        store
            .migrate_legacy(&options)
            .await
            .map_err(EneRuntimeError::Memory)
    }

    /// Destructive legacy memory reset for a character card.
    pub async fn reset_legacy_memory(&self, card_name: &str) -> Result<(), EneRuntimeError> {
        let store = self.require_store()?;
        store
            .reset_legacy_memory(card_name)
            .await
            .map_err(EneRuntimeError::Memory)
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
            store,
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

    /// Search typed memories and attach explainable recall reasons (#74 / #123).
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
            store,
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
    pub async fn transition_typed_memory_status(
        &self,
        id: i64,
        status: ene_store::MemoryStatus,
    ) -> Result<bool, EneRuntimeError> {
        let store = self.require_store()?;
        store
            .transition_typed_memory_status(id, status)
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
            EneRuntimeError::Memory(ene_store::MemoryError::MemoryStoreConnectionError(
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
}

/// Event receiver for [`DiagnosticEvent`].
pub struct DiagnosticEventReceiver(broadcast::Receiver<DiagnosticEvent>);

impl std::fmt::Debug for DiagnosticEventReceiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticEventReceiver").finish()
    }
}

impl DiagnosticEventReceiver {
    /// Non-blocking poll.
    pub fn try_recv(&mut self) -> Result<DiagnosticEvent, broadcast::error::TryRecvError> {
        self.0.try_recv()
    }

    /// Async receive.
    pub async fn recv(&mut self) -> Result<DiagnosticEvent, broadcast::error::RecvError> {
        self.0.recv().await
    }
}

/// Concrete diagnostics facade returned by [`crate::EneHandle::diagnostics`].
pub struct EneDiagnostics {
    pub(crate) cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    pub(crate) diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub(crate) memory: MemoryQueryHandle,
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

    /// Subscribe to diagnostic events (pipeline phases/metrics).
    pub fn subscribe(&self) -> DiagnosticEventReceiver {
        DiagnosticEventReceiver(self.diag_tx.subscribe())
    }

    /// Request a snapshot of the current actor state.
    pub async fn get_snapshot(&self) -> Result<EneStateSnapshot, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::GetSnapshot { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)
    }

    /// List available tools from the registry.
    pub async fn list_tools(&self) -> Result<Vec<ToolSpec>, EneRuntimeError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ListTools { reply: tx })
            .map_err(|_| EneRuntimeError::ChannelClosed)?;
        rx.await.map_err(|_| EneRuntimeError::ChannelClosed)
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
    pub fn invalidate_tool_index(&self) -> Result<(), ActorDeadError> {
        self.cmd_tx
            .send(EneCommand::InvalidateToolIndex)
            .map_err(|_| ActorDeadError)
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
    let _ = diag_tx.send(event);
}
