//! Durable Action attempts: identity, operation kind, certainty, and the
//! repository boundary (AU5, K-H, E-2).
//!
//! One attempt is one logical try. It is inserted as [`ActionCertainty::Unknown`]
//! and its certainty changes only through the compare-and-set transition; a
//! retry is always a new [`ActionAttemptId`], never a continued attempt. The
//! row's existence is a correlation, not proof that the action ran, succeeded,
//! or that an agent is alive.
//!
//! The durable types in this module carry cross-domain identities as opaque
//! values ([`RawId`] / [`RevisionInner`]); they never name another domain's
//! newtype. The meaning of a Permission evaluation stays with the Permission
//! owner: the orchestration boundary reduces an issued evaluation to its
//! [`RawId`] before the premise is built.

use ene_primitive::{RawId, RevisionInner, WallClockWithTz};
use thiserror::Error;

/// Identity of one Action attempt. Wraps [`RawId`]; never reused.
///
/// A retry, resend, or re-execution of an unknown outcome starts a new
/// identity; an existing identity is never replayed or continued.
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

/// The kind of external operation one attempt performs.
///
/// The closed world for this slice is list/read/create/edit: the Stage 4
/// workspace boundary permits listing, reading, creating, and editing only.
/// Delete and execute require their producers (Owner confirmation, extension
/// acceptance) and are added by the slice that can produce them; there is no
/// inert variant for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationKind {
    /// Non-recursive directory enumeration.
    List,
    Read,
    Create,
    Edit,
}

impl OperationKind {
    /// Stable storage name, closed world.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Read => "read",
            Self::Create => "create",
            Self::Edit => "edit",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world.
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

/// What ene has confirmed about one attempt's external effect.
///
/// [`Self::Unknown`] is a stable state, not a missing value: it is retained
/// until new objective evidence arrives. Cancellation acceptance, transport
/// success, screen display, durable-write success, reconnect, restore, and
/// client movement never promote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionCertainty {
    /// Confirmed success: the intended change was observed at the target.
    ConfirmedSuccess,
    /// Confirmed failure: the operation refused or failed before changing the
    /// target, or the target could be observed unchanged.
    ConfirmedFailure,
    /// The effect cannot be confirmed. Held until objective evidence arrives.
    Unknown,
}

impl ActionCertainty {
    /// Stable storage name, closed world.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedSuccess => "confirmed_success",
            Self::ConfirmedFailure => "confirmed_failure",
            Self::Unknown => "unknown",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world.
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

/// The objective basis of one certainty update, closed world.
///
/// An agent's "I succeeded" self-report is not a basis and has no variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectGrounds {
    /// The executor itself observed the result at the resolved target
    /// (for example, a read-back matching the intended content).
    ObservedAtTarget,
    /// The operation was refused or failed before any change could occur, and
    /// the target is known untouched.
    RefusedBeforeEffect,
    /// An effect may have occurred, but it could not be confirmed. Keeps the
    /// attempt at [`ActionCertainty::Unknown`].
    OutcomeUnverified,
}

impl EffectGrounds {
    /// Stable storage name, closed world.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObservedAtTarget => "observed_at_target",
            Self::RefusedBeforeEffect => "refused_before_effect",
            Self::OutcomeUnverified => "outcome_unverified",
        }
    }

    /// Parses the [`Self::as_str`] vocabulary, closed world.
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

/// Whether one `(certainty, grounds)` pair is allowed by the closed world.
///
/// The started attempt has no grounds yet; every reported pair must agree:
/// confirmed success is observed, confirmed failure is known to be a
/// pre-effect refusal, and an unverified outcome stays unknown.
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

/// The resolved target of one external operation.
///
/// Constructed by the execution-time resolution
/// ([`WorkspaceRoot::resolve`](crate::WorkspaceRoot::resolve)); the public
/// constructor exists only for the store's durable read-back and for tests,
/// and a caller must never build it from an input string to establish
/// identity. Forgery is contained: the effect entry
/// (`WorkspaceRoot::execute`) is `pub(crate)`, so only
/// [`orchestrate_workspace_action`](crate::orchestrate_workspace_action) can
/// execute, and effect time re-verifies containment and the
/// mount/reparse/volume boundary fail-closed for every operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealTargetRef(String);

impl RealTargetRef {
    /// Wraps an already-resolved canonical path.
    ///
    /// The caller vouches that resolution and containment already happened;
    /// this constructor performs no filesystem access.
    #[must_use]
    pub fn from_canonical_path(path: String) -> Self {
        Self(path)
    }

    /// The canonical target path.
    #[must_use]
    pub fn as_path(&self) -> &str {
        &self.0
    }
}

/// The full premise of one attempt insertion (AU5).
///
/// The orchestration mints `attempt`; the repository never re-allocates it.
/// The correlation values are owner-defined opaque values (CM §4.3).
/// `relied_evaluation` is the opaque identity of the K-B.1 single-use judgment
/// this start relies on; the repository refuses a second attempt using the
/// same identity. The judgment itself is Permission-owned: this type holds
/// only its raw correlation identity, never the Permission newtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptCommitPremise {
    pub attempt: ActionAttemptId,
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    /// The current workspace association identity, as re-read at start time.
    pub workspace: RawId,
    /// The target resolved immediately before the start request.
    pub real_target: RealTargetRef,
    pub operation: OperationKind,
    /// The opaque identity of the evaluation that authorized exactly this
    /// use. Action stores only this durable correlation; the evaluation's
    /// meaning and single-use tracking stay with the Permission owner.
    pub relied_evaluation: RawId,
}

/// The domain result of one attempt insertion.
///
/// Every non-`Started` variant is an `Ok`-side domain outcome with zero
/// writes and zero execution; a missing delegation, task, or workspace
/// association, and a moved revision or association all answer
/// `StalePremise` (the Work owner re-reads to distinguish them). Terminal
/// Task progress and an execution-sealed delegation are their own Action-owned
/// outcomes: Action never imports the Task lifecycle type, and the Work-side
/// adapter re-reads the durable state to explain them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStartOutcome {
    /// The attempt row is durable; execution may proceed outside any
    /// transaction under exactly this premise.
    Started,
    /// The delegation/task/workspace premise is no longer current; nothing
    /// was written and nothing may be executed.
    StalePremise,
    /// The Task is terminal (`Completed` / `Failed`); nothing was written and
    /// no external effect may happen.
    TaskTerminal,
    /// The delegation already submitted its final result (execution seal);
    /// nothing was written and no external effect may happen, even while the
    /// Task is non-terminal.
    ExecutionSealed,
}

/// The domain result of one certainty compare-and-set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertaintyUpdateOutcome {
    /// The expected value matched and the new value was stored.
    Updated,
    /// The stored certainty is no longer `expected`; nothing was changed.
    StaleCurrent { current: ActionCertainty },
    /// No attempt row exists for the identity; nothing was changed.
    MissingAttempt,
}

/// Technical failure of Action persistence; never a domain outcome.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ActionTechnicalError {
    #[error("action storage unavailable: {reason}")]
    StorageUnavailable {
        /// Backend-supplied cause. Never body content or secret material.
        reason: String,
    },
}

/// The durable correlation of one attempt as read back after restart.
///
/// Reading a row never re-claims, replays, or re-executes the operation; the
/// row only says an attempt was started under a verified premise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionAttemptRecord {
    pub attempt: ActionAttemptId,
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub real_target: RealTargetRef,
    pub operation: OperationKind,
    /// The opaque identity of the K-B.1 evaluation this attempt started
    /// under, as durable correlation only; Action never reconstructs the
    /// Permission-owned evaluation from it.
    pub relied_evaluation: RawId,
    pub certainty: ActionCertainty,
    /// `None` only for an attempt that has not reported an observation yet.
    pub grounds: Option<EffectGrounds>,
    pub started_at: WallClockWithTz,
}

/// Durable boundary for Action attempts (AU5, SD-Attempt).
///
/// [`Self::insert_attempt_if_current`] is the start linearization point that
/// compares the delegation correspondence, the relied task revision, the
/// current task revision, the current workspace association, and the
/// delegation's copied scope association in one short `Immediate`
/// transaction; no await or external I/O happens inside it.
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
    /// before any external effect. A premise that disagrees with the stored
    /// delegation correspondence, duplicate workspace association rows,
    /// unknown operation names, and a duplicate attempt identity are
    /// technical errors (fail closed, never reduced to stale).
    async fn insert_attempt_if_current(
        &self,
        premise: AttemptCommitPremise,
    ) -> Result<ActionStartOutcome, ActionTechnicalError>;

    /// Compare-and-sets one attempt's certainty from `expected`.
    ///
    /// Only `expected == Unknown` is accepted, and only the closed-world
    /// `(new, grounds)` pairs are stored; a confirmed value is never
    /// rewritten. The operation is one short transaction, never held across
    /// I/O.
    async fn compare_and_set_certainty(
        &self,
        attempt: ActionAttemptId,
        expected: ActionCertainty,
        new: ActionCertainty,
        grounds: EffectGrounds,
    ) -> Result<CertaintyUpdateOutcome, ActionTechnicalError>;

    /// Loads one attempt's durable correlation by identity.
    ///
    /// `None` means no row exists. Malformed or internally inconsistent rows
    /// are technical errors and are never composed into a record.
    async fn load_attempt(
        &self,
        attempt: ActionAttemptId,
    ) -> Result<Option<ActionAttemptRecord>, ActionTechnicalError>;
}

#[cfg(test)]
mod tests {
    use super::{
        ActionAttemptId, ActionCertainty, EffectGrounds, OperationKind,
        certainty_grounds_pair_is_valid,
    };

    #[test]
    fn generated_attempt_ids_are_distinct_and_stable() {
        let first = ActionAttemptId::generate();
        let second = ActionAttemptId::generate();
        assert_ne!(first, second);
        assert_eq!(ActionAttemptId::from_raw(first.as_raw()), first);
    }

    #[test]
    fn operation_names_round_trip_as_a_closed_world() {
        for operation in [
            OperationKind::List,
            OperationKind::Read,
            OperationKind::Create,
            OperationKind::Edit,
        ] {
            assert_eq!(
                OperationKind::from_name(operation.as_str()),
                Some(operation)
            );
        }
        assert_eq!(OperationKind::from_name("delete"), None);
        assert_eq!(OperationKind::from_name("execute"), None);
        assert_eq!(OperationKind::from_name(""), None);
    }

    #[test]
    fn certainty_and_grounds_names_round_trip_as_a_closed_world() {
        for certainty in [
            ActionCertainty::ConfirmedSuccess,
            ActionCertainty::ConfirmedFailure,
            ActionCertainty::Unknown,
        ] {
            assert_eq!(
                ActionCertainty::from_name(certainty.as_str()),
                Some(certainty)
            );
        }
        for grounds in [
            EffectGrounds::ObservedAtTarget,
            EffectGrounds::RefusedBeforeEffect,
            EffectGrounds::OutcomeUnverified,
        ] {
            assert_eq!(EffectGrounds::from_name(grounds.as_str()), Some(grounds));
        }
        assert_eq!(ActionCertainty::from_name("confirmed"), None);
        assert_eq!(EffectGrounds::from_name("agent_reported_success"), None);
    }

    #[test]
    fn only_the_closed_world_certainty_grounds_pairs_are_valid() {
        assert!(certainty_grounds_pair_is_valid(
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::ObservedAtTarget
        ));
        assert!(certainty_grounds_pair_is_valid(
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::RefusedBeforeEffect
        ));
        assert!(certainty_grounds_pair_is_valid(
            ActionCertainty::Unknown,
            EffectGrounds::OutcomeUnverified
        ));
        assert!(!certainty_grounds_pair_is_valid(
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::OutcomeUnverified
        ));
        assert!(!certainty_grounds_pair_is_valid(
            ActionCertainty::ConfirmedSuccess,
            EffectGrounds::RefusedBeforeEffect
        ));
        assert!(!certainty_grounds_pair_is_valid(
            ActionCertainty::ConfirmedFailure,
            EffectGrounds::ObservedAtTarget
        ));
    }
}
