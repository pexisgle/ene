//! Targeted Deletion admission and unfinished-operation contracts.
//!
//! A1 deliberately provides no public confirmation mint and no transition to
//! finalizing/completed. A1b supplies the trusted first-party issuer
//! ([`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command));
//! the verified completion boundary (A5) must supply completion, not caller
//! booleans.

use ene_primitive::{RawId, WallClockWithTz};
use zeroize::Zeroizing;

use crate::{
    ConfirmTargetedDeletionOutcome, DeletionMaterialOutcome, DeletionOperationId,
    DeletionParticipantRecord, DeletionRequestId, DeletionSurfaceMark, DeletionSweepGeneration,
    ErasureConditionRef, ParticipantCompletionFact, ParticipantCompletionOutcome,
    ParticipantDemandOutcome, ParticipantOwnerRef, StageTargetedDeletionRequestCommand,
    StageTargetedDeletionRequestOutcome, TargetedDeletionRequest,
};

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

    /// Host-local Owner review only (IPC §18.1 preview): the trusted console
    /// may show the exact text before the Owner confirms. Never a log,
    /// `Debug`, wire, or management-view representation.
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
    /// Exploration aids only. Never replace mechanical matching.
    pub semantic_hints: Vec<DeletionSearchMaterial>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletionPurpose {
    Privacy,
    Security,
}

impl DeletionPurpose {
    /// Storage and display token of the closed purpose set. Unknown stored or
    /// incoming tokens are outside the set and fail closed at their parse
    /// boundary, never defaulted.
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

/// Sealed, request-bound evidence. No Deserialize, raw-ID constructor, or
/// public fields: Client/LLM output cannot manufacture final confirmation.
/// The only production mint site is
/// [`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command),
/// which requires the store-read staged request *and* its durable Host-local
/// confirmation fact.
///
/// ```compile_fail
/// use ene_preservation::TrustedOwnerConfirmationRef;
/// let confirmation = TrustedOwnerConfirmationRef {};
/// ```
///
/// ```compile_fail
/// use ene_preservation::{DeletionPurpose, StartTargetedDeletionCommand, TargetedDeletionTarget,
///     MechanicalDeletionTarget, DeletionSearchMaterial};
/// let forged = StartTargetedDeletionCommand::confirmed(
///     ene_primitive::RawId::new(),
///     TargetedDeletionTarget {
///         mechanical: MechanicalDeletionTarget::ExactText(DeletionSearchMaterial::new(
///             String::from("target"),
///         )),
///         semantic_hints: Vec::new(),
///     },
///     DeletionPurpose::Privacy,
///     ene_primitive::WallClockWithTz::now(),
/// );
/// ```
#[derive(Debug, Clone)]
pub struct TrustedOwnerConfirmationRef {
    request: RawId,
}

/// Immutable request: confirmation cannot be transferred to a changed target,
/// purpose, or source scope. Known correlations come from semantic owners,
/// never from a Client's choice of database rows. The required participant set
/// is the current product surface the composition decides on; this crate does
/// not enumerate capabilities.
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
    /// Required participant snapshot for the operation being admitted. The
    /// durable set must be non-empty and duplicate-free: an operation with no
    /// required participants could be completed without any erasure, so it is
    /// refused rather than treated as vacuously complete.
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

    /// Crate-internal mint for the durable-confirmed path (A1b).
    ///
    /// Only [`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command)
    /// calls this, and only with both a store-read staged request and a
    /// store-read durable confirmation fact. No public constructor,
    /// `Deserialize`, or caller boolean exists on this path.
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

    /// Test-only evidence, absent from production builds. Production reaches
    /// confirmation only through
    /// [`TargetedDeletionRequest::into_command`](crate::TargetedDeletionRequest::into_command)
    /// with a durable Host-local request and its durable confirmation fact.
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
    /// The required participant set is empty or repeats an owner. Either shape
    /// would make the durable snapshot ambiguous, and an empty set would let an
    /// operation be completed without any erasure.
    #[error("invalid required participant set")]
    InvalidParticipantSet,
    /// The requested deletion operation does not exist. An unknown operation
    /// is never an authoritative empty participant set.
    #[error("unknown deletion operation")]
    UnknownOperation,
}

/// Canonical persistence boundary. Admission publishes operation, protected
/// material, initial condition, known source correlations, and the required
/// participant snapshot atomically before returning Started. No participant
/// effects occur within these methods.
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
    /// SELECT-only, keyset-paged at the database boundary; limit is 1..=100.
    /// The page is ordered by stored owner name, and the operation's rows are
    /// validated before they are returned, so torn participant state fails
    /// closed instead of reading as an incomplete set. An unknown operation is
    /// [`PreservationTechnicalError::UnknownOperation`], never an empty set.
    fn deletion_participants(
        &self,
        operation: DeletionOperationId,
        after: Option<ParticipantOwnerRef>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionParticipantRecord>, PreservationTechnicalError>,
    > + Send;

    /// Protected operation-lifetime material for one fan-out pass. Reads no
    /// body once the operation completed and its material was destroyed.
    fn deletion_operation_material(
        &self,
        operation: DeletionOperationId,
    ) -> impl std::future::Future<
        Output = Result<DeletionMaterialOutcome, PreservationTechnicalError>,
    > + Send;

    /// Durably marks one required participant `Running` for the operation's
    /// current sweep before any participant effect starts. The write is
    /// idempotent for the same `(operation, sweep, participant)` and refuses
    /// to regress a sweep that already reached `Verified`.
    fn begin_participant_demand(
        &self,
        condition: ErasureConditionRef,
        participant: ParticipantOwnerRef,
    ) -> impl std::future::Future<
        Output = Result<ParticipantDemandOutcome, PreservationTechnicalError>,
    > + Send;

    /// Records one completion fact against the current sweep only. A fact from
    /// an older generation never updates current state, an owner outside the
    /// durable snapshot is never registered lazily, and a verified sweep is
    /// terminal: a later downgrading report cannot reopen it.
    fn record_participant_completion(
        &self,
        fact: ParticipantCompletionFact,
    ) -> impl std::future::Future<
        Output = Result<ParticipantCompletionOutcome, PreservationTechnicalError>,
    > + Send;

    /// Stages one advisory Targeted Deletion request (lifecycle §15).
    ///
    /// Nothing is enforced by staging: no condition is published and no
    /// operation exists until the Owner's trusted Host-local confirmation
    /// runs [`Self::start_confirmed_targeted_deletion`]. An identical staged
    /// request is returned as-is instead of minting a second one.
    fn stage_targeted_deletion(
        &self,
        command: StageTargetedDeletionRequestCommand,
    ) -> impl std::future::Future<
        Output = Result<StageTargetedDeletionRequestOutcome, PreservationTechnicalError>,
    > + Send;
    /// Host-local trusted confirmation inlet (IPC §18.1): records the Owner's
    /// final confirmation for one staged request and then runs the canonical
    /// admission for it.
    ///
    /// Idempotent by request identity: a duplicate confirmation observes the
    /// same single operation and never creates a second one. A missing
    /// request answers [`ConfirmTargetedDeletionOutcome::Missing`] and writes
    /// nothing. `required_participants` is the Host composition's current
    /// product-surface owner set (lifecycle §8); the admission transaction
    /// snapshots it durably with the operation.
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
    fn start_confirmed_targeted_deletion(
        &self,
        request: DeletionRequestId,
        required_participants: Vec<ParticipantOwnerRef>,
    ) -> impl std::future::Future<
        Output = Result<StartTargetedDeletionOutcome, PreservationTechnicalError>,
    > + Send;
    /// SELECT-only, keyset-paged staged requests still awaiting the Host-local
    /// confirmation; limit is 1..=100. Confirmed requests are excluded: their
    /// operation is read through [`Self::deletion_status`].
    fn pending_targeted_deletions(
        &self,
        after: Option<DeletionRequestId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<TargetedDeletionRequest>, PreservationTechnicalError>,
    > + Send;
    /// SELECT-only, keyset-paged operation status *including terminal phases*;
    /// limit is 1..=100. This is the status view's read: it never returns
    /// protected material, and a torn page fails closed instead of dropping
    /// rows.
    fn deletion_status(
        &self,
        after: Option<DeletionOperationId>,
        limit: u32,
    ) -> impl std::future::Future<
        Output = Result<Vec<DeletionOperationRecord>, PreservationTechnicalError>,
    > + Send;
    /// Current display-revision mark of the deletion surface, derived from the
    /// canonical request and operation rows. Comparison material only.
    fn deletion_surface_mark(
        &self,
    ) -> impl std::future::Future<Output = Result<DeletionSurfaceMark, PreservationTechnicalError>> + Send;
}
