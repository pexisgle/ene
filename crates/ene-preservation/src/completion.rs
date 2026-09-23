use ene_primitive::WallClockWithTz;

use crate::{
    DeletionHoldReason, DeletionOperationId, DeletionOperationRef, DeletionPurpose,
    DeletionSweepGeneration, ParticipantOwnerRef,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionCompletionSummary {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
    pub required: u64,
    pub verified: u64,
    pub local_complete: u64,
    pub in_progress: u64,
    pub held: u64,
}

impl DeletionCompletionSummary {
    #[must_use]
    pub const fn all_verified(&self) -> bool {
        self.required > 0 && self.verified == self.required
    }

    #[must_use]
    pub const fn is_well_formed(&self) -> bool {
        self.local_complete + self.in_progress + self.held + self.verified == self.required
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionAuditStatus {
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionAuditParticipant {
    pub owner: ParticipantOwnerRef,
    pub status: DeletionAuditStatus,
    pub erased_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCompletionAudit {
    pub operation: DeletionOperationId,
    pub purpose: DeletionPurpose,
    pub started_at: WallClockWithTz,
    pub completed_at: WallClockWithTz,
    pub sweep_count: u64,
    pub erased_count: u64,
    pub participants: Vec<DeletionAuditParticipant>,
}

impl DeletionCompletionAudit {
    #[must_use]
    pub fn verified_count(&self) -> u64 {
        u64::try_from(self.participants.len()).unwrap_or(u64::MAX)
    }
}
