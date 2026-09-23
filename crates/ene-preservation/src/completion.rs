//! Global completion vocabulary for Targeted Deletion (lifecycle §10, §12-§14).
//!
//! Local completion and system-wide completion are different facts. A
//! participant's `LocalComplete` (or one successful transaction, one Client
//! ACK, or one LLM self-report) is never a global completion candidate: only a
//! durable aggregate of the whole required participant set for the operation's
//! *current* sweep can be, and the completion boundary re-derives that premise
//! from the canonical store instead of accepting a caller boolean.
//!
//! This module owns the vocabulary only. The state transition, the
//! system-wide mechanical remainder verification, and the atomic
//! material-wipe / audit / condition-closure commit live in the canonical
//! store (`PreservationRepository`).

use crate::{
    DeletionHoldReason, DeletionOperationId, DeletionOperationRef, DeletionSweepGeneration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionCompletionSummary {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
    pub required: u64,
    pub verified: u64,
}

impl DeletionCompletionSummary {
    #[must_use]
    pub const fn all_verified(&self) -> bool {
        self.required > 0 && self.verified == self.required
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionFinalizationOutcome {
    Finalizing,
    Completed,
    CompletedAlready,
    NotVerified(DeletionCompletionSummary),
    NotFinalizing,
    RemainderCollected(DeletionOperationRef),
    Held(DeletionHoldReason),
    Missing,
    StaleSweep,
    UnverifiableMaterial,
    ReconciliationIncomplete,
}

pub const DELETION_RECONCILIATION_PAGE_SIZE: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionReconciliationOutcome {
    Advanced,
    Complete,
    Finalizing,
    Completed,
    Missing,
    StaleSweep,
}
