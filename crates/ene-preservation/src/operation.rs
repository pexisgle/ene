//! Targeted Deletion admission and unfinished-operation contracts.
//!
//! A1 deliberately provides no public confirmation mint. A1b supplies the
//! trusted first-party issuer
//! ([`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command));
//! A5 supplies the sealed finalizing/completion boundary
//! ([`PreservationRepository::begin_deletion_finalizing`] /
//! [`PreservationRepository::complete_deletion_finalizing`]), whose premise is
//! the durable participant aggregate and the system-wide mechanical remainder
//! verification — never a caller boolean.

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

/// Immutable request: the target, purpose, and required participant snapshot
/// are fixed at mint, so a caller cannot retarget or widen an admitted
/// operation. The required participant set is the current product surface the
/// composition decides on; this crate does not enumerate capabilities.
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
    /// Required participant snapshot for the operation being admitted. The
    /// durable set must be non-empty and duplicate-free: an operation with no
    /// required participants could be completed without any erasure, so it is
    /// refused rather than treated as vacuously complete.
    #[must_use]
    pub fn required_participants(&self) -> &[ParticipantOwnerRef] {
        &self.required_participants
    }

    /// Crate-internal mint for the durable-confirmed path (A1b).
    ///
    /// Only [`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command)
    /// calls this, and only with a store-read staged request whose Owner
    /// confirmation row the admission transaction re-reads. No public
    /// constructor, `Deserialize`, or caller boolean exists on this path.
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
    /// Storage and display token of the closed phase set. Unknown stored or
    /// incoming tokens are outside the set and fail closed at their parse
    /// boundary, never defaulted.
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
    /// Storage and display token of the closed hold-reason set. Unknown stored
    /// or incoming tokens are outside the set and fail closed at their parse
    /// boundary, never defaulted.
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

/// One bounded walk over the unfinished deletion operations that keeps a
/// durable scheduling cursor.
///
/// A cursor is a position in the keyset of unfinished operations, never
/// completion, verification, or hold truth: every walk re-derives the phase,
/// hold, and participant status of each operation it visits from the
/// canonical rows, and a cursor naming a completed or vanished operation is
/// only the start position of the next page. The walks are separate because
/// each makes an independent decision over the same unfinished set: the
/// fan-out drives operations, the hold scan decides which holds to offer a
/// resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionWalk {
    /// The Host fan-out pass over unfinished operations.
    FanOut,
    /// The retryable-hold resume scan (`Held(Unavailable)`).
    RetryableHold,
}

impl DeletionWalk {
    /// Storage token of the closed walk set. An unknown stored token is torn
    /// state, never a defaulted walk.
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

/// Canonical persistence boundary. Admission publishes operation, protected
/// material, initial condition, known source correlations, and the required
/// participant snapshot atomically before returning Started. No participant
/// effects occur within these methods. On the first-party request path the
/// known source correlations are enumerated from the owner's durable identity
/// rows whose text carries the confirmed exact target (lifecycle §4.1 point
/// 4).
///
/// Source-correlation invariant for the erasure-currentness hot path: an
/// unfinished operation keeps `erasure_condition_source` rows only in its
/// current sweep, and a completed operation keeps zero source rows. The
/// completion boundary (A5; A1 exposes no completion authority) must close
/// the current condition and delete the operation's material, hints, and all
/// source rows atomically — historical `erasure_condition` rows may remain,
/// but no source copy is kept for audit/history. Any remaining source row for
/// a completed operation is canonical corruption and fails closed.
///
/// Participant invariant (same canonical store, no second registry): every
/// operation carries a non-empty required participant snapshot from admission;
/// every participant row tracks the operation's current sweep; a completed
/// operation has every participant `Verified` for that sweep. Any other shape
/// is canonical corruption and fails closed.
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
    /// The durable scheduling position of one unfinished-operation walk, or
    /// `None` when the next page starts at the beginning.
    ///
    /// The position is a rotation cursor, never completion truth: it is the
    /// operation id a previous bounded pass examined last, and no operation
    /// state is ever derived from it. A missing row is a fresh walk at the
    /// beginning, not an empty unfinished set.
    fn deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
    ) -> impl std::future::Future<
        Output = Result<Option<DeletionOperationId>, PreservationTechnicalError>,
    > + Send;
    /// Persists the scheduling position of one walk after a bounded pass
    /// examined operations up to `after`; `None` restarts the next pass at the
    /// beginning.
    ///
    /// This writes only the cursor: it never changes an operation phase,
    /// participant state, condition, or completion premise, and it is safe to
    /// call with a stale position (the next pass re-derives everything it
    /// visits).
    fn set_deletion_walk_cursor(
        &self,
        walk: DeletionWalk,
        after: Option<DeletionOperationId>,
    ) -> impl std::future::Future<Output = Result<(), PreservationTechnicalError>> + Send;
    /// Same canonical current set used by AU14 and Task resume. No sentinel.
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
    /// Canonical admission for a request whose Owner confirmation is already
    /// durable (crash recovery, and the intent path observing a confirmed
    /// request). Adds no authority: without the durable confirmation row this
    /// answers [`StartTargetedDeletionOutcome::ConfirmationRequired`].
    ///
    /// The admission transaction writes the operation, its protected material,
    /// the initial condition, the required participant snapshot, and a durable
    /// reconciliation cursor per known identity table, then publishes the
    /// first bounded pages of the source correlations already known at
    /// admission (lifecycle §4.1 point 4): the implementation enumerates the
    /// owner's durable identity rows whose stored text carries the confirmed
    /// exact target, in bounded pages, and associates the already-claimed uses
    /// each page covers. A Client, model output, or caller never names a
    /// source on this path. The page bound is a work bound, not a correctness
    /// bound: the durable cursor lets
    /// [`Self::reconcile_deletion_sources`] walk every remaining covered
    /// identity to its end, and global completion refuses while that walk is
    /// incomplete.
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

    /// Sealed finalizing transition (§12).
    ///
    /// The caller supplies only the expected operation ref. The store re-reads
    /// the durable participant aggregate inside the write transaction and
    /// refuses unless **every** required participant is `Verified` for the
    /// operation's *current* sweep; there is no boolean, token, or
    /// self-reported premise that can substitute for that durable state. The
    /// same transaction re-reads the current sweep's covered-source
    /// reconciliation state and refuses with
    /// [`DeletionFinalizationOutcome::ReconciliationIncomplete`] while any
    /// known identity table is still being walked: the already-claimed
    /// in-flight uses the completion must account for are only all durable
    /// once the exhaustive walk finished, and an arbitrary page bound is never
    /// a completion premise (§4.1 point 4, §18).
    ///
    /// Before entering `Finalizing`, the same transaction runs the
    /// system-wide mechanical remainder verification (LLM-independent, over
    /// the closed canonical content surface). If it finds still-collected
    /// target data, the completion is abandoned *before* any material is
    /// destroyed: a new sweep generation opens, every participant resets to
    /// pending, and the outcome is
    /// [`DeletionFinalizationOutcome::RemainderCollected`].
    ///
    /// The transition is idempotent for an operation already `Finalizing`.
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
