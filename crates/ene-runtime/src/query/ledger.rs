//! Interactive memory/commitment ledger handle.
//!
//! Reads (`list_memories` / `inspect_memory` / `list_commitments`) are
//! mailbox-free: they only touch `MemoryStore`, never the actor's
//! turn-execution state, so they never queue behind an in-flight `Run` turn.
//! Mutations (edit / salience adjustment) route through the actor mailbox as
//! [`crate::handle::EneCommand`] variants carrying the active `TurnId`, which
//! serializes them with turn execution and lets the actor emit the
//! [`LifecycleEvent::MemoryLedgerChanged`](crate::handle::event::LifecycleEvent::MemoryLedgerChanged)
//! audit event on the lifecycle bus. The actor arm is the single mutation
//! surface for the ledger, which is also where the L1 recall cache must
//! invalidate on edit / salience changes (edits also refresh the stored
//! embeddings so vector recall does not serve stale text).
//!
//! Deletion (forget) and commitment lifecycle (complete / cancel) are not
//! duplicated here — they already exist on [`crate::diagnostics::MemoryHandle`].

use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use crate::types::TurnId;
use ene_store::{Commitment, CommitmentStatus, MemoryEdit, MemoryItem, MemoryJournalListOptions};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Handle over the interactive memory/commitment ledger
/// (list / inspect / edit / salience adjustment).
///
/// Obtained via [`crate::EneHandle::memory_ledger`]. Cheap to clone (wraps an
/// optional `Arc` plus a shared card-name lock).
#[derive(Clone)]
pub struct MemoryLedgerHandle {
    store: Option<Arc<ene_store::MemoryStore>>,
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    /// Current character-card name, kept in sync by the turn actor whenever
    /// the character card is swapped (`SetCharacter`). Reading it here never
    /// requires a mailbox round-trip.
    card_name: Arc<parking_lot::Mutex<String>>,
}

impl MemoryLedgerHandle {
    pub(crate) const fn new(
        store: Option<Arc<ene_store::MemoryStore>>,
        cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
        card_name: Arc<parking_lot::Mutex<String>>,
    ) -> Self {
        Self {
            store,
            cmd_tx,
            card_name,
        }
    }

    fn store(&self) -> Result<&Arc<ene_store::MemoryStore>, PublicApiError> {
        self.store.as_ref().ok_or_else(|| PublicApiError::Internal {
            message: "Memory store is not enabled".to_string(),
        })
    }

    /// List typed memories for the ledger with journal-style filters.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    pub async fn list_memories(
        &self,
        options: &MemoryJournalListOptions<'_>,
    ) -> Result<Vec<MemoryItem>, PublicApiError> {
        self.store()?
            .list_journal_memories(options)
            .await
            .map_err(PublicApiError::from)
    }

    /// Inspect a single typed memory by id.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`. `Ok(None)` when no
    /// such memory exists.
    pub async fn inspect_memory(&self, id: i64) -> Result<Option<MemoryItem>, PublicApiError> {
        self.store()?
            .get_typed_memory(id)
            .await
            .map_err(PublicApiError::from)
    }

    /// List commitments for the current character in every lifecycle status.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    pub async fn list_commitments(
        &self,
        user_id: Option<&str>,
        status: Option<CommitmentStatus>,
        limit: usize,
    ) -> Result<Vec<Commitment>, PublicApiError> {
        let character_id = self.card_name.lock().clone();
        self.store()?
            .list_commitments(&character_id, user_id, status, limit)
            .await
            .map_err(PublicApiError::from)
    }

    /// Edit a persisted typed memory in place.
    ///
    /// Validation happens in the store before any write, so an invalid edit
    /// leaves the original memory untouched. Routes through the actor mailbox
    /// with the active `TurnId` and emits a
    /// [`LifecycleEvent::MemoryLedgerChanged`](crate::handle::event::LifecycleEvent::MemoryLedgerChanged)
    /// audit event on success; the actor also refreshes the row's embeddings
    /// in the background so vector recall does not serve stale text.
    pub async fn edit_memory(
        &self,
        id: i64,
        edit: MemoryEdit,
        turn: Option<TurnId>,
    ) -> Result<(), PublicApiError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::EditMemory {
                id,
                edit,
                turn,
                reply: reply_tx,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        reply_rx.await.map_err(|_| PublicApiError::ActorDead)?
    }

    /// Set the salience (importance / Preference weight) of a typed memory.
    ///
    /// The value is clamped into `0.0..=1.0` by the store. Routes through the
    /// actor mailbox with the active `TurnId` and emits a
    /// [`LifecycleEvent::MemoryLedgerChanged`](crate::handle::event::LifecycleEvent::MemoryLedgerChanged)
    /// audit event on success.
    pub async fn set_memory_salience(
        &self,
        id: i64,
        salience: f32,
        turn: Option<TurnId>,
    ) -> Result<(), PublicApiError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::SetMemorySalience {
                id,
                salience,
                turn,
                reply: reply_tx,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        reply_rx.await.map_err(|_| PublicApiError::ActorDead)?
    }
}
