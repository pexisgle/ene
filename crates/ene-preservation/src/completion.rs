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
