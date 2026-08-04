//! Pending memory-candidate approval flow.
//!
//! Reads (`list_pending` / `inspect_pending` / `history`) are mailbox-free:
//! they only touch `MemoryStore`, never the actor's turn-execution state, so
//! they never queue behind an in-flight `Run` turn. Mutations (approve /
//! edit / reject) route through the actor mailbox as [`EneCommand`]
//! variants carrying the active `TurnId`, which serializes them with turn
//! execution and lets the actor emit the
//! [`LifecycleEvent::CandidateChanged`](crate::handle::event::LifecycleEvent::CandidateChanged)
//! audit event on the lifecycle bus. The actor arms are the single mutation
//! surface for the queue, which is also where the L1 recall cache must
//! invalidate on approve / reject / edit (approve persists a typed memory,
//! reject removes a pending row, and edits change the title/content that
//! feed lexical pending recall).

use crate::handle::EneCommand;
use crate::public_api::PublicApiError;
use crate::types::TurnId;
use ene_store::{MemoryStore, PendingCandidate, PendingCandidateStatus};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Re-export so consumers can name the edit payload without reaching into
/// `ene-store`.
pub use ene_store::PendingCandidateEdit;

/// Summary of a pending memory candidate for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingCandidateSummary {
    /// Database primary key.
    pub id: i64,
    /// Short title or label.
    pub title: String,
    /// Full candidate content.
    pub content: String,
    /// Memory kind as string (e.g. "episodic", "semantic").
    pub kind: String,
    /// Confidence score (0.0 .. 1.0).
    pub confidence: f32,
    /// Human-readable reason for the extraction.
    pub reason_detail: String,
    /// Title of an existing memory this candidate would supersede, if any.
    pub existing_memory_title: Option<String>,
    /// Id of the existing memory this candidate would supersede, if any.
    pub existing_memory_id: Option<i64>,
    /// Source quote from the conversation that triggered this candidate.
    pub source_quote: String,
    /// Source turn that triggered this candidate, when available.
    pub source_turn: Option<String>,
    /// Workflow status as string (`pending` / `approved` / `rejected`).
    pub status: String,
    /// When the candidate was created (RFC 3339).
    pub created_at: String,
    /// When the candidate was approved or rejected (RFC 3339), if resolved.
    pub resolved_at: Option<String>,
}

impl From<&PendingCandidate> for PendingCandidateSummary {
    fn from(c: &PendingCandidate) -> Self {
        Self {
            id: c.id,
            title: c.title.clone(),
            content: c.content.clone(),
            kind: c.kind.as_str().to_string(),
            confidence: c.confidence,
            reason_detail: c.reason_detail.clone(),
            existing_memory_title: c.existing_memory_title.clone(),
            existing_memory_id: c.existing_memory_id,
            source_quote: c.source_quote.clone(),
            source_turn: c.source_turn.clone(),
            status: c.status.as_str().to_string(),
            created_at: c.created_at.to_rfc3339(),
            resolved_at: c.resolved_at.map(|ts| ts.to_rfc3339()),
        }
    }
}

/// Handle over the pending memory-candidate approval flow
/// (list / inspect / history / approve / edit / reject).
///
/// Obtained via [`crate::EneHandle::candidates`]. Cheap to clone (wraps an
/// optional `Arc` plus a shared card-name lock).
#[derive(Clone)]
pub struct MemoryCandidateHandle {
    store: Option<Arc<MemoryStore>>,
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    /// Current character-card name, kept in sync by the turn actor whenever
    /// the character card is swapped (`SetCharacter`). Reading it here never
    /// requires a mailbox round-trip.
    card_name: Arc<parking_lot::Mutex<String>>,
}

impl MemoryCandidateHandle {
    pub(crate) const fn new(
        store: Option<Arc<MemoryStore>>,
        cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
        card_name: Arc<parking_lot::Mutex<String>>,
    ) -> Self {
        Self {
            store,
            cmd_tx,
            card_name,
        }
    }

    fn store(&self) -> Result<&Arc<MemoryStore>, PublicApiError> {
        self.store.as_ref().ok_or_else(|| PublicApiError::Internal {
            message: "Memory store is not enabled".to_string(),
        })
    }

    /// List pending memory candidates awaiting user approval.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    pub async fn list_pending(&self) -> Result<Vec<PendingCandidateSummary>, PublicApiError> {
        let character_id = self.card_name.lock().clone();
        let list = self
            .store()?
            .list_pending_candidates(&character_id, Some(PendingCandidateStatus::Pending))
            .await?;
        let mut summaries: Vec<PendingCandidateSummary> =
            list.iter().map(PendingCandidateSummary::from).collect();
        self.resolve_existing_titles(&mut summaries).await;
        Ok(summaries)
    }

    /// Inspect a single pending candidate (any status) by id.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`. `Ok(None)` when no
    /// such candidate exists.
    pub async fn inspect_pending(
        &self,
        id: i64,
    ) -> Result<Option<PendingCandidateSummary>, PublicApiError> {
        let Some(candidate) = self.store()?.get_pending_candidate(id).await? else {
            return Ok(None);
        };
        let mut summary = PendingCandidateSummary::from(&candidate);
        self.resolve_existing_titles(std::slice::from_mut(&mut summary))
            .await;
        Ok(Some(summary))
    }

    /// List resolved candidates (approved / rejected), newest first.
    ///
    /// Part of the API v1 contract: errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`. The resolved
    /// queue is bounded by the retention sweep (`pending_candidate_retention`).
    pub async fn history(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingCandidateSummary>, PublicApiError> {
        let character_id = self.card_name.lock().clone();
        let store = self.store()?;
        let mut rows = store
            .list_pending_candidates(&character_id, Some(PendingCandidateStatus::Approved))
            .await?;
        rows.extend(
            store
                .list_pending_candidates(&character_id, Some(PendingCandidateStatus::Rejected))
                .await?,
        );
        // Resolved history reads most naturally in resolution order; rows
        // written before `resolved_at` existed fall back to creation time.
        rows.sort_by(|a, b| {
            b.resolved_at
                .unwrap_or(b.created_at)
                .cmp(&a.resolved_at.unwrap_or(a.created_at))
                .then_with(|| b.id.cmp(&a.id))
        });
        rows.truncate(limit);
        let mut summaries: Vec<PendingCandidateSummary> =
            rows.iter().map(PendingCandidateSummary::from).collect();
        self.resolve_existing_titles(&mut summaries).await;
        Ok(summaries)
    }

    /// Approve a pending memory candidate, persisting it as a typed memory.
    ///
    /// Routes through the actor mailbox with the active `TurnId` and emits a
    /// [`LifecycleEvent::CandidateChanged`](crate::handle::event::LifecycleEvent::CandidateChanged)
    /// audit event on success.
    pub async fn approve(&self, id: i64, turn: Option<TurnId>) -> Result<(), PublicApiError> {
        self.send_resolution(id, turn, PendingCandidateStatus::Approved)
            .await
    }

    /// Reject a pending memory candidate.
    ///
    /// Routes through the actor mailbox with the active `TurnId` and emits a
    /// [`LifecycleEvent::CandidateChanged`](crate::handle::event::LifecycleEvent::CandidateChanged)
    /// audit event on success.
    pub async fn reject(&self, id: i64, turn: Option<TurnId>) -> Result<(), PublicApiError> {
        self.send_resolution(id, turn, PendingCandidateStatus::Rejected)
            .await
    }

    /// Edit a still-pending candidate's user-editable fields.
    ///
    /// Validation happens in the store before any write, so an invalid edit
    /// leaves the original candidate untouched. Routes through the actor
    /// mailbox with the active `TurnId` and emits a
    /// [`LifecycleEvent::CandidateChanged`](crate::handle::event::LifecycleEvent::CandidateChanged)
    /// audit event on success.
    pub async fn edit(
        &self,
        id: i64,
        edit: PendingCandidateEdit,
        turn: Option<TurnId>,
    ) -> Result<(), PublicApiError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::EditCandidate {
                id,
                edit,
                turn,
                reply: reply_tx,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        reply_rx.await.map_err(|_| PublicApiError::ActorDead)?
    }

    async fn send_resolution(
        &self,
        id: i64,
        turn: Option<TurnId>,
        status: PendingCandidateStatus,
    ) -> Result<(), PublicApiError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(EneCommand::ResolveCandidate {
                id,
                turn,
                status,
                reply: reply_tx,
            })
            .map_err(|_| PublicApiError::ActorDead)?;
        reply_rx.await.map_err(|_| PublicApiError::ActorDead)?
    }

    /// Resolve conflict titles for rows rehydrated from the DB, where the
    /// denormalized `existing_memory_title` is always `None`.
    async fn resolve_existing_titles(&self, summaries: &mut [PendingCandidateSummary]) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        for summary in summaries {
            if summary.existing_memory_title.is_none()
                && let Some(existing_id) = summary.existing_memory_id
                && let Ok(Some(existing)) = store.get_typed_memory(existing_id).await
            {
                summary.existing_memory_title = Some(existing.title);
            }
        }
    }
}
