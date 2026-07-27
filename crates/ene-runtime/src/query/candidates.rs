//! Pending memory-candidate approval flow (#174, #223, #271).
//!
//! See the [`crate::query`] module docs for why this bypasses the actor
//! mailbox entirely: candidate approval only ever touches `MemoryStore`,
//! never the actor's turn-execution state.

use crate::public_api::PublicApiError;
use ene_store::{MemoryStore, PendingCandidate, PendingCandidateStatus};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Summary of a pending memory candidate for the UI (#174 / #223).
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
    /// Source quote from the conversation that triggered this candidate.
    pub source_quote: String,
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
            source_quote: c.source_quote.clone(),
        }
    }
}

/// Handle over the pending memory-candidate approval flow
/// (list / approve / reject).
///
/// Obtained via [`crate::EneHandle::candidates`]. Cheap to clone (wraps an
/// optional `Arc` plus a small shared card-name lock).
#[derive(Clone)]
pub struct MemoryCandidateHandle {
    store: Option<Arc<MemoryStore>>,
    /// Current character-card name, kept in sync by the turn actor whenever
    /// the character card is swapped (`SetCharacter`). Reading it here never
    /// requires a mailbox round-trip.
    card_name: Arc<parking_lot::Mutex<String>>,
}

impl MemoryCandidateHandle {
    pub(crate) const fn new(
        store: Option<Arc<MemoryStore>>,
        card_name: Arc<parking_lot::Mutex<String>>,
    ) -> Self {
        Self { store, card_name }
    }

    fn store(&self) -> Result<&Arc<MemoryStore>, PublicApiError> {
        self.store.as_ref().ok_or_else(|| PublicApiError::Internal {
            message: "Memory store is not enabled".to_string(),
        })
    }

    /// List pending memory candidates awaiting user approval (#174).
    ///
    /// Part of the API v1 contract (#274): errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    #[expect(
        clippy::unused_async,
        reason = "kept async for API symmetry with approve/reject, which do await the store; \
                   a future recall-aware listing may also need to await"
    )]
    pub async fn list_pending(&self) -> Result<Vec<PendingCandidateSummary>, PublicApiError> {
        let character_id = self.card_name.lock().clone();
        let list = self
            .store()?
            .list_pending_candidates(&character_id, Some(PendingCandidateStatus::Pending))?;
        Ok(list.iter().map(PendingCandidateSummary::from).collect())
    }

    /// Approve a pending memory candidate (#174).
    ///
    /// Part of the API v1 contract (#274): errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    pub async fn approve(&self, id: i64) -> Result<(), PublicApiError> {
        self.store()?.approve_pending_candidate(id).await?;
        Ok(())
    }

    /// Reject a pending memory candidate (#174).
    ///
    /// Part of the API v1 contract (#274): errors are the stable
    /// [`PublicApiError`] categories, not a bare `String`.
    pub async fn reject(&self, id: i64) -> Result<(), PublicApiError> {
        self.store()?.resolve_pending_candidate(id, false).await?;
        Ok(())
    }
}
