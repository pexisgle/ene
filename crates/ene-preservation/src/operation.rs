//! Targeted Deletion admission and unfinished-operation contracts.
//!
//! A1 deliberately provides no public confirmation mint and no transition to
//! finalizing/completed. The trusted first-party issuer (A1b) and verified
//! completion boundary (A5) must supply those authorities, not caller booleans.

use ene_primitive::{RawId, WallClockWithTz};
use zeroize::Zeroizing;

use crate::{DeletionOperationId, DeletionSweepGeneration, ErasureConditionRef};

/// Operation-lifetime material; never an audit field or management payload.
#[derive(Clone, PartialEq, Eq)]
pub struct DeletionSearchMaterial(Zeroizing<String>);

impl std::fmt::Debug for DeletionSearchMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeletionSearchMaterial([REDACTED])")
    }
}

impl DeletionSearchMaterial {
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(Zeroizing::new(text))
    }

    /// Protected owner/storage access, not a display representation.
    #[must_use]
    pub fn expose_for_erasure(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MechanicalDeletionTarget {
    ExactText(DeletionSearchMaterial),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetedDeletionTarget {
    pub mechanical: MechanicalDeletionTarget,
    /// Exploration aids only. Never replace mechanical matching.
    pub semantic_hints: Vec<DeletionSearchMaterial>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionPurpose {
    Privacy,
    Security,
}

/// Sealed, request-bound evidence. No Deserialize, raw-ID constructor, or
/// public fields: Client/LLM output cannot manufacture final confirmation.
/// The first-party issuer is deliberately not part of the A1 store slice.
///
/// ```compile_fail
/// use ene_preservation::TrustedOwnerConfirmationRef;
/// let confirmation = TrustedOwnerConfirmationRef {};
/// ```
#[derive(Debug, Clone)]
pub struct TrustedOwnerConfirmationRef {
    request: RawId,
}

/// Immutable request: confirmation cannot be transferred to a changed target,
/// purpose, or source scope. Known correlations come from semantic owners,
/// never from a Client's choice of database rows.
#[derive(Debug, Clone)]
pub struct StartTargetedDeletionCommand {
    request: RawId,
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    requested_at: WallClockWithTz,
    known_sources: Vec<RawId>,
    confirmation: Option<TrustedOwnerConfirmationRef>,
}

impl StartTargetedDeletionCommand {
    #[must_use]
    pub fn new(
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
        known_sources: Vec<RawId>,
    ) -> Self {
        Self {
            request: RawId::new(),
            target,
            purpose,
            requested_at,
            known_sources,
            confirmation: None,
        }
    }

    #[must_use]
    pub fn target(&self) -> &TargetedDeletionTarget {
        &self.target
    }
    #[must_use]
    pub fn purpose(&self) -> DeletionPurpose {
        self.purpose
    }
    #[must_use]
    pub fn requested_at(&self) -> WallClockWithTz {
        self.requested_at
    }
    #[must_use]
    pub fn known_sources(&self) -> &[RawId] {
        &self.known_sources
    }
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmation
            .as_ref()
            .is_some_and(|fact| fact.request == self.request)
    }

    /// Test-only evidence, absent from production builds. A1b must implement
    /// the actual Host-local trusted issuer before user-facing admission.
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn confirmed_for_tests(mut self) -> Self {
        self.confirmation = Some(TrustedOwnerConfirmationRef {
            request: self.request,
        });
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionOperationPhase {
    Active,
    Held,
    Finalizing,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionHoldReason {
    Unavailable,
    GenerationExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionOperationRef {
    pub operation: DeletionOperationId,
    pub sweep: DeletionSweepGeneration,
}

impl DeletionOperationRef {
    #[must_use]
    pub fn condition(self) -> ErasureConditionRef {
        ErasureConditionRef {
            operation: self.operation,
            sweep: self.sweep,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionOperationRecord {
    pub current: DeletionOperationRef,
    pub phase: DeletionOperationPhase,
    pub purpose: DeletionPurpose,
    pub started_at: WallClockWithTz,
    pub hold: Option<DeletionHoldReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartTargetedDeletionOutcome {
    Started(DeletionOperationRef),
    NeedsClarification,
    ConfirmationRequired,
    AlreadyCoveredBy(DeletionOperationRef),
    HeldByOperation(DeletionOperationRef),
}

/// Only unfinished transitions. Verification/closure are intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionLifecycleChange {
    Hold,
    Resume,
    NextSweep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionLifecycleOutcome {
    Applied(DeletionOperationRef),
    Missing,
    StaleSweep,
    Completed,
    Held(DeletionHoldReason),
    Finalizing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentErasureCondition {
    pub condition: ErasureConditionRef,
    /// The operation identity is also its scope identity; no parallel scope registry.
    pub scope: DeletionOperationId,
    pub opened_at: WallClockWithTz,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreservationTechnicalError {
    #[error("preservation storage unavailable")]
    StorageUnavailable,
    #[error("corrupt preservation state")]
    CorruptState,
    #[error("invalid preservation query limit")]
    InvalidLimit,
}

/// Canonical persistence boundary. Admission publishes operation, protected
/// material, initial condition and known source correlations atomically before
/// returning Started. No participant effects occur within these methods.
///
/// Source-correlation invariant for the erasure-currentness hot path: an
/// unfinished operation keeps `erasure_condition_source` rows only in its
/// current sweep, and a completed operation keeps zero source rows. The
/// completion boundary (A5; A1 exposes no completion authority) must close
/// the current condition and delete the operation's material, hints, and all
/// source rows atomically — historical `erasure_condition` rows may remain,
/// but no source copy is kept for audit/history. Any remaining source row for
/// a completed operation is canonical corruption and fails closed.
pub trait PreservationRepository: Send + Sync {
    fn start_targeted_deletion(
        &self,
        command: StartTargetedDeletionCommand,
    ) -> impl std::future::Future<
        Output = Result<StartTargetedDeletionOutcome, PreservationTechnicalError>,
    > + Send;
    fn change_deletion_lifecycle(
        &self,
        expected: DeletionOperationRef,
        change: DeletionLifecycleChange,
    ) -> impl std::future::Future<
        Output = Result<DeletionLifecycleOutcome, PreservationTechnicalError>,
    > + Send;
    /// SELECT-only, keyset-paged at the database boundary; limit is 1..=100.
    fn unfinished_deletions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionOperationRecord>, PreservationTechnicalError>,
    > + Send;
    /// Same canonical current set used by AU14 and Task resume. No sentinel.
    fn current_erasure_conditions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<CurrentErasureCondition>, PreservationTechnicalError>,
    > + Send;
}
