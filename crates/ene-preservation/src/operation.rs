use ene_primitive::WallClockWithTz;
use zeroize::Zeroizing;

use crate::{
    ConfirmTargetedDeletionOutcome, DeletionFinalizationOutcome, DeletionMaterialOutcome,
    DeletionOperationId, DeletionParticipantRecord, DeletionReconciliationOutcome,
    DeletionRequestId, DeletionSurfaceMark, DeletionSweepGeneration, ErasureConditionRef,
    ParticipantCompletionFact, ParticipantCompletionOutcome, ParticipantDemandOutcome,
    ParticipantOwnerRef, StageTargetedDeletionRequestCommand, StageTargetedDeletionRequestOutcome,
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
pub struct StartTargetedDeletionCommand {
    target: TargetedDeletionTarget,
    purpose: DeletionPurpose,
    requested_at: WallClockWithTz,
    required_participants: Vec<ParticipantOwnerRef>,
}

impl StartTargetedDeletionCommand {
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
    pub fn required_participants(&self) -> &[ParticipantOwnerRef] {
        &self.required_participants
    }

    #[must_use]
    pub(crate) fn confirmed(
        target: TargetedDeletionTarget,
        purpose: DeletionPurpose,
        requested_at: WallClockWithTz,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> Self {
        Self {
            target,
            purpose,
            requested_at,
            required_participants,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionOperationPhase {
    Active,
    Held,
    Finalizing,
    Completed,
}

impl DeletionOperationPhase {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Held => "held",
            Self::Finalizing => "finalizing",
            Self::Completed => "completed",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "active" => Self::Active,
            "held" => Self::Held,
            "finalizing" => Self::Finalizing,
            "completed" => Self::Completed,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionHoldReason {
    Unavailable,
    GenerationExhausted,
}

impl DeletionHoldReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::GenerationExhausted => "generation_exhausted",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "unavailable" => Self::Unavailable,
            "generation_exhausted" => Self::GenerationExhausted,
            _ => return None,
        })
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionWalk {
    FanOut,
    RetryableHold,
}

impl DeletionWalk {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FanOut => "fan_out",
            Self::RetryableHold => "retryable_hold",
        }
    }
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
    fn deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
    ) -> impl std::future::Future<
        Output = Result<Option<DeletionOperationId>, PreservationTechnicalError>,
    > + Send;
    fn set_deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
        after: Option<DeletionOperationId>,
    ) -> impl std::future::Future<Output = Result<(), PreservationTechnicalError>> + Send;
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
}
