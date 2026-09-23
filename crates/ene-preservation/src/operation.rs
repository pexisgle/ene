use ene_primitive::{RawId, WallClockWithTz};
use zeroize::Zeroizing;

use crate::{
    ConfirmTargetedDeletionOutcome, DeletionCompletionAudit, DeletionCompletionSummary,
    DeletionFinalizationOutcome, DeletionMaterialOutcome, DeletionOperationId,
    DeletionParticipantRecord, DeletionReconciliationOutcome, DeletionRequestId,
    DeletionSurfaceMark, DeletionSweepGeneration, ErasureConditionRef, ParticipantCompletionFact,
    ParticipantCompletionOutcome, ParticipantDemandOutcome, ParticipantOwnerRef,
    StageTargetedDeletionRequestCommand, StageTargetedDeletionRequestOutcome,
    TargetedDeletionRequest,
};

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

    #[must_use]
    pub fn expose_for_erasure(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn expose_for_owner_review(&self) -> &str {
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
    pub semantic_hints: Vec<DeletionSearchMaterial>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionPurpose {
    Privacy,
    Security,
}

impl DeletionPurpose {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Privacy => "privacy",
            Self::Security => "security",
        }
    }

    #[must_use]
    pub fn from_name(token: &str) -> Option<Self> {
        match token {
            "privacy" => Some(Self::Privacy),
            "security" => Some(Self::Security),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrustedOwnerConfirmationRef {
    request: RawId,
}

#[derive(Debug, Clone)]
pub struct StartTargetedDeletionCommand {
    request: RawId,
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    requested_at: WallClockWithTz,
    known_sources: Vec<RawId>,
    required_participants: Vec<ParticipantOwnerRef>,
    confirmation: Option<TrustedOwnerConfirmationRef>,
}

impl StartTargetedDeletionCommand {
    #[must_use]
    pub fn new(
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
        known_sources: Vec<RawId>,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Self {
        Self {
            request: RawId::new(),
            target,
            purpose,
            requested_at,
            known_sources,
            required_participants,
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
    pub fn required_participants(&self) -> &[ParticipantOwnerRef] {
        &self.required_participants
    }
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.confirmation
            .as_ref()
            .is_some_and(|fact| fact.request == self.request)
    }

    #[must_use]
    pub(crate) fn confirmed(
        request: RawId,
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Self {
        Self {
            request,
            target,
            purpose,
            requested_at,
            known_sources: Vec::new(),
            required_participants,
            confirmation: Some(TrustedOwnerConfirmationRef { request }),
        }
    }

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
    #[error("invalid required participant set")]
    InvalidParticipantSet,
    #[error("unknown deletion operation")]
    UnknownOperation,
}

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
    fn unfinished_deletions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionOperationRecord>, PreservationTechnicalError>,
    > + Send;
    fn current_erasure_conditions(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<CurrentErasureCondition>, PreservationTechnicalError>,
    > + Send;
    fn deletion_participants(
        &self,
        operation: DeletionOperationId,
        after: Option<ParticipantOwnerRef>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionParticipantRecord>, PreservationTechnicalError>,
    > + Send;

    fn deletion_operation_material(
        &self,
        operation: DeletionOperationId,
    ) -> impl std::future::Future<
        Output = Result<DeletionMaterialOutcome, PreservationTechnicalError>,
    > + Send;

    fn begin_participant_demand(
        &self,
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
    ) -> impl std::future::Future<
        Output = Result<ParticipantDemandOutcome, PreservationTechnicalError>,
    > + Send;

    fn record_participant_completion(
        &self,
        fact: ParticipantCompletionFact,
    ) -> impl std::future::Future<
        Output = Result<ParticipantCompletionOutcome, PreservationTechnicalError>,
    > + Send;

    fn stage_targeted_deletion(
        &self,
        command: StageTargetedDeletionRequestCommand,
    ) -> impl std::future::Future<
        Output = Result<StageTargetedDeletionRequestOutcome, PreservationTechnicalError>,
    > + Send;
    fn confirm_targeted_deletion(
        &self,
        request: DeletionRequestId,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> impl std::future::Future<
        Output = Result<ConfirmTargetedDeletionOutcome, PreservationTechnicalError>,
    > + Send;
    fn start_confirmed_targeted_deletion(
        &self,
        request: DeletionRequestId,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> impl std::future::Future<
        Output = Result<StartTargetedDeletionOutcome, PreservationTechnicalError>,
    > + Send;
    fn reconcile_deletion_sources(
        &self,
        expected: DeletionOperationRef,
        page_size: u32,
    ) -> impl std::future::Future<
        Output = Result<DeletionReconciliationOutcome, PreservationTechnicalError>,
    > + Send;
    fn pending_targeted_deletions(
        &self,
        after: Option<DeletionRequestId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<TargetedDeletionRequest>, PreservationTechnicalError>,
    > + Send;
    fn deletion_status(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionOperationRecord>, PreservationTechnicalError>,
    > + Send;
    fn deletion_surface_mark(
        &self,
    ) -> impl std::future::Future<Output = Result<DeletionSurfaceMark, PreservationTechnicalError>> + Send;

    fn deletion_completion_summary(
        &self,
        operation: DeletionOperationId,
    ) -> impl std::future::Future<
        Output = Result<DeletionCompletionSummary, PreservationTechnicalError>,
    > + Send;

    fn begin_deletion_finalizing(
        &self,
        expected: DeletionOperationRef,
    ) -> impl std::future::Future<
        Output = Result<DeletionFinalizationOutcome, PreservationTechnicalError>,
    > + Send;

    fn complete_deletion_finalizing(
        &self,
        expected: DeletionOperationRef,
    ) -> impl std::future::Future<
        Output = Result<DeletionFinalizationOutcome, PreservationTechnicalError>,
    > + Send;

    fn deletion_completion_audit(
        &self,
        operation: DeletionOperationId,
    ) -> impl std::future::Future<
        Output = Result<Option<DeletionCompletionAudit>, PreservationTechnicalError>,
    > + Send;
}
