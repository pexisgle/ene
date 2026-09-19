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

use ene_primitive::WallClockWithTz;

use crate::{
    DeletionHoldReason, DeletionOperationId, DeletionOperationRef, DeletionPurpose,
    DeletionSweepGeneration, ParticipantOwnerRef,
};

/// Durable aggregate of one operation's required participant set for its
/// current sweep (§10).
///
/// The counts are a snapshot the completion boundary re-reads inside its own
/// write transaction; a summary observed by a caller is never authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionCompletionSummary {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
    /// Required participant rows in the durable admission snapshot. Always
    /// non-empty by the admission invariant.
    pub required: u64,
    /// Rows whose current-sweep state is `Verified`.
    pub verified: u64,
    /// Rows that finished their bounded erase pass but not their remainder
    /// check.
    pub local_complete: u64,
    /// Rows still `Pending` or `Running`.
    pub in_progress: u64,
    /// Rows `Held` for the current sweep.
    pub held: u64,
}

impl DeletionCompletionSummary {
    /// The only durable participant premise a global completion candidate may
    /// rest on: every required participant `Verified` for exactly this sweep.
    ///
    /// One local completion, one successful transaction, one Client ACK, or
    /// one LLM self-report is never substituted for this (§10).
    #[must_use]
    pub const fn all_verified(&self) -> bool {
        self.required > 0 && self.verified == self.required
    }

    /// Whether every required participant row is accounted for exactly once.
    #[must_use]
    pub const fn is_well_formed(&self) -> bool {
        self.local_complete + self.in_progress + self.held + self.verified == self.required
    }
}

/// Outcome of one sealed finalizing / completion step (§12).
///
/// Every premise is re-derived from durable state. No variant is reachable by
/// a caller's self-report: the same generic outcome is returned whether the
/// premise failed because of a pending participant, a hold, or a concurrent
/// generation advance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionFinalizationOutcome {
    /// The operation is durably `Finalizing`: local erasure and the current
    /// sweep's verification are complete, and the material wipe / audit /
    /// condition closure commit is owed. The current condition is still
    /// active (§5.1).
    Finalizing,
    /// The completion commit ran: the body-free audit is durable, the current
    /// condition is closed, the operation's protected material and every
    /// source correlation are destroyed, and the phase is `Completed`.
    Completed,
    /// The operation was already `Completed`; nothing changed. A completed
    /// operation is terminal and never resumed (§5.1).
    CompletedAlready,
    /// Not every required participant is `Verified` for the current sweep, so
    /// no finalization step applies. The operation stays active (§10).
    NotVerified(DeletionCompletionSummary),
    /// The operation's phase is neither `Finalizing` nor `Completed`, so no
    /// completion step applies. Nothing changed.
    NotFinalizing,
    /// The §12 step-1 re-check found still-collected target data (a delayed
    /// arrival or remainder that reached durable storage after verification).
    /// **No protected material was destroyed**: a new sweep generation is
    /// open, every participant is pending for it, and the operation is
    /// `Active` again (§6/§12).
    RemainderCollected(DeletionOperationRef),
    /// A generation advance was impossible or the operation is held; the
    /// operation keeps its current condition and never completes (§5.1/§6).
    Held(DeletionHoldReason),
    /// No such operation.
    Missing,
    /// The caller's expected operation ref is no longer the operation's
    /// current generation: a stale result never advances or completes the
    /// operation (§6/§14).
    StaleSweep,
    /// A `Finalizing` operation has no readable protected material, so the
    /// system-wide mechanical remainder probe cannot run. Completion fails
    /// closed instead of guessing that target text is gone (§12).
    UnverifiableMaterial,
    /// The current sweep's covered-source reconciliation has not finished
    /// walking every known identity table, so the already-claimed in-flight
    /// uses the completion must account for are not all durable yet. Entering
    /// `Finalizing` is refused: the operation stays `Active` and the bounded
    /// reconciliation pages must finish first (§4.1 point 4, §12 §18).
    ReconciliationIncomplete,
}

/// Default page size of one bounded reconciliation step.
///
/// The bound is a work bound, never a correctness bound: a page that fills
/// exactly is continued from its durable cursor, and the last page of a table
/// is the one that found fewer covered identities than the page size. No
/// covered identity is dropped because a table held more than one page.
pub const DELETION_RECONCILIATION_PAGE_SIZE: u32 = 64;

/// One bounded step of the exhaustive covered-source reconciliation
/// (lifecycle §4.1 point 4, §12 step 1).
///
/// Admission publishes a first bounded page of the covered source
/// correlations and initializes a durable per-identity-table cursor; the
/// remaining pages are driven by bounded calls to
/// [`crate::PreservationRepository::reconcile_deletion_sources`]. Correctness is the
/// exhaustive walk, never the page bound: global completion requires the
/// current sweep's reconciliation to be `Complete`, and a source identity is
/// never dropped because it fell past a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionReconciliationOutcome {
    /// One bounded page was published and associated. `Advanced` always means
    /// work was done: the caller drives the next bounded step, which answers
    /// [`Self::Complete`] when the durable cursors show the walk is over
    /// (possibly a page that finished the last table — the completion is
    /// observed from the cursors, never inferred from a page shape).
    Advanced,
    /// Every known identity table has been walked to its end for the current
    /// sweep. The in-flight-use correspondence for the already-claimed uses is
    /// durable, and the completion premise may be attempted.
    Complete,
    /// The operation is durably `Finalizing`: by invariant reconciliation was
    /// complete before the marker was taken. Nothing changed.
    Finalizing,
    /// The operation already `Completed`. A completed operation is terminal
    /// and never reconciled again.
    Completed,
    /// No such operation.
    Missing,
    /// The caller's expected operation ref is no longer the operation's
    /// current generation: a stale step never advances reconciliation (§6).
    StaleSweep,
}

/// Final audited status of one required participant (§13).
///
/// A completed operation has every required participant `Verified` — the only
/// terminal state this vocabulary admits — so a hold can never be written into
/// a completion audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionAuditStatus {
    Verified,
}

/// One body-free audit entry per required participant owner (§13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionAuditParticipant {
    pub owner: ParticipantOwnerRef,
    pub status: DeletionAuditStatus,
    pub erased_count: u64,
}

/// Body-free completion audit (§13).
///
/// Carries objective metadata only: the operation identity, the purpose class,
/// the start and completion times, the sweep count, every required
/// participant's final status, and the erased/verified counts. It never
/// carries the target body, a reversible encoding of it, a target
/// hash/fingerprint, a search token, a credential value, or a prompt/output
/// body — none of those are fields, and none are derived into one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCompletionAudit {
    pub operation: DeletionOperationId,
    pub purpose: DeletionPurpose,
    pub started_at: WallClockWithTz,
    pub completed_at: WallClockWithTz,
    /// Final sweep generation. Generations start at 1 and advance one per
    /// sweep, so this is also the operation's sweep count.
    pub sweep_count: u64,
    /// Total rows erased across every sweep of the operation.
    pub erased_count: u64,
    /// One entry per required participant of the durable snapshot.
    pub participants: Vec<DeletionAuditParticipant>,
}

impl DeletionCompletionAudit {
    /// Number of participants whose final status is `Verified`. Equal to
    /// [`Self::participants`] length by the completion invariant.
    #[must_use]
    pub fn verified_count(&self) -> u64 {
        u64::try_from(self.participants.len()).unwrap_or(u64::MAX)
    }
}
