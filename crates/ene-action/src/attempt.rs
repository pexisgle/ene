use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionAttemptId(RawId);

impl ActionAttemptId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationKind {
    List,
    Read,
    Create,
    Edit,
}

impl OperationKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::Create => "create",
            Self::Edit => "edit",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "list" => Some(Self::List),
            "read" => Some(Self::Read),
            "create" => Some(Self::Create),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCertainty {
    ConfirmedSuccess,
    ConfirmedFailure,
    Unknown,
}

impl ActionCertainty {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedSuccess => "confirmed_success",
            Self::ConfirmedFailure => "confirmed_failure",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "confirmed_success" => Some(Self::ConfirmedSuccess),
            "confirmed_failure" => Some(Self::ConfirmedFailure),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectGrounds {
    ObservedAtTarget,
    RefusedBeforeEffect,
    OutcomeUnverified,
}

impl EffectGrounds {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObservedAtTarget => "observed_at_target",
            Self::RefusedBeforeEffect => "refused_before_effect",
            Self::OutcomeUnverified => "outcome_unverified",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "observed_at_target" => Some(Self::ObservedAtTarget),
            "refused_before_effect" => Some(Self::RefusedBeforeEffect),
            "outcome_unverified" => Some(Self::OutcomeUnverified),
            _ => None,
        }
    }
}

#[must_use]
pub fn certainty_grounds_pair_is_valid(certainty: ActionCertainty, grounds: EffectGrounds) -> bool {
    matches!(
        (certainty, grounds),
        (
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget
        ) | (
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::RefusedBeforeEffect
        ) | (ActionCertainty::Unknown, EffectGrounds::OutcomeUnverified)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealTargetRef(String);

impl RealTargetRef {
    #[must_use]
    pub fn from_canonical_path(path: String) -> Self {
        Self(path)
    }

    #[must_use]
    pub fn as_path(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptCommitPremise {
    pub attempt: ActionAttemptId,
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub real_target: RealTargetRef,
    pub operation: OperationKind,
    pub relied_evaluation: RawId,
}

/// The domain result of one attempt insertion.
///
/// Every non-`Started` variant is an `Ok`-side domain outcome that writes no
/// attempt row and executes nothing; a missing delegation, task, or workspace
/// association, and a moved revision or association all answer
/// `StalePremise` (the Work owner re-reads to distinguish them). Terminal
/// Task progress and an execution-sealed delegation are their own Action-owned
/// outcomes: Action never imports the Task lifecycle type, and the Work-side
/// adapter re-reads the durable state to explain them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStartOutcome {
    Started,
    StalePremise,
    TaskTerminal,
    ExecutionSealed,
    /// A canonical current erasure condition covers the resolved target
    /// (lifecycle §7/§11). No attempt row is written and no external effect may
    /// start: the attempt is refused before it exists, so no target copy is
    /// saved and no outcome has to be retracted. The coverage gate's erasure-use
    /// hold is nonetheless committed (materialized when it was still
    /// unreconciled), so the correspondence outlives the refused try.
    ///
    /// Distinct from [`Self::StalePremise`] (a correlation moved) and
    /// [`Self::TaskTerminal`] / [`Self::ExecutionSealed`] (the Task or
    /// delegation closed): the target itself is under an active deletion.
    HeldForErasure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertaintyUpdateOutcome {
    Updated,
    StaleCurrent { current: ActionCertainty },
    MissingAttempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ActionTechnicalError {
    #[error("action storage unavailable: {reason}")]
    StorageUnavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionAttemptRecord {
    pub attempt: ActionAttemptId,
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub real_target: RealTargetRef,
    pub operation: OperationKind,
    pub relied_evaluation: RawId,
    pub certainty: ActionCertainty,
    pub grounds: Option<EffectGrounds>,
    pub started_at: WallClockWithTz,
}

#[expect(
    async_fn_in_trait,
    reason = "Stage 4 contract style uses native async fn; Send bounds settle with the store impl"
)]
pub trait ActionAttemptRepository: Send + Sync {
    /// Inserts one attempt iff every durable premise still holds.
    ///
    /// Missing rows and moved revisions/associations answer
    /// [`ActionStartOutcome::StalePremise`] without writes; terminal Task
    /// progress answers [`ActionStartOutcome::TaskTerminal`] and an
    /// execution-sealed delegation answers
    /// [`ActionStartOutcome::ExecutionSealed`], both without writes and
    /// before any external effect. A canonical current erasure condition
    /// covering the resolved target answers
    /// [`ActionStartOutcome::HeldForErasure`]: no attempt row is written and
    /// no external effect starts, but the gate's erasure-use hold is
    /// committed so the correspondence outlives the refused try. A premise
    /// that disagrees with the stored delegation correspondence, duplicate
    /// workspace association rows, unknown operation names, and a duplicate
    /// attempt identity are technical errors (fail closed, never reduced to
    /// stale).
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise,
    ) -> Result<ActionStartOutcome, ActionTechnicalError>;

    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError>;

    async fn load_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError>;
}
