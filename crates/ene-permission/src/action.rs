//! Minimal Action authorization for the Stage 4 workspace capability (K-B.1).
//!
//! The inference [`PermissionEvaluationId`](crate::PermissionEvaluationId) /
//! [`EvaluationTracker`](crate::EvaluationTracker) are deliberately not reused:
//! this module owns a separate candidate, premise, single-use evaluation, and
//! tracker. A decision binds the exact use it was issued for — delegation,
//! relied Task revision, workspace association, operation, and the resolved
//! real target — so it can never be replayed for a different target or
//! operation.
//!
//! The current premise is supplied by the caller after re-reading the Task
//! owner's durable state. A stored copy is never a live grant; the explicit
//! arm here is the closed Stage 4 workspace capability (`List / Read / Create /
//! Edit`) for a delegated Task revision. Rules, Deny, Always-ask, and owner
//! questions arrive with their producers and extend [`authorize_action_use`]
//! only through explicit arms, never by default-allow.

use std::collections::HashMap;

use ene_primitive::{RawId, RevisionInner};

/// Permission-owned operation vocabulary for the Stage 4 workspace capability.
///
/// Delete and execute are absent on purpose: their Owner-confirmation and
/// extension producers do not exist yet, and no inert variant is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// Non-recursive directory enumeration.
    List,
    /// Read one regular file.
    Read,
    /// Create one new regular file.
    Create,
    /// Replace one existing regular file.
    Edit,
}

impl ActionKind {
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

/// Identity of one Action-use authorization decision.
///
/// Separate from the inference [`PermissionEvaluationId`](crate::PermissionEvaluationId).
/// Minted only by [`ActionEvaluationTracker`] for one candidate fingerprint;
/// single-use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionPermissionEvaluationId(RawId);

impl ActionPermissionEvaluationId {
    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

/// The specific action use one decision binds.
///
/// `resolved_target` is the canonical target resolved immediately before the
/// start request; binding it means a decision is never reusable for a
/// different object even when the requested path text coincides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionUseCandidate {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
    pub operation: ActionKind,
    pub resolved_target: String,
}

impl ActionUseCandidate {
    fn fingerprint(&self) -> ActionEvalFingerprint {
        ActionEvalFingerprint(
            self.delegation,
            self.task,
            self.task_revision,
            self.workspace,
            self.operation,
            self.resolved_target.clone(),
        )
    }
}

/// Closed-world fingerprint one Action evaluation id is bound to.
///
/// Tuple order is `(delegation, task, task_revision, workspace, operation,
/// resolved_target)`. Kept crate-private: callers consume with the candidate
/// itself.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActionEvalFingerprint(RawId, RawId, RevisionInner, RawId, ActionKind, String);

/// The current premise the caller re-read from the Task owner's durable state.
///
/// Passing this into the policy is not a grant: a durable copy is comparison
/// material, and the AU5 start transaction re-checks the same durable values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentActionPremise {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
}

/// Result of one Action-use authorization query.
///
/// `NeedsRevalidation` means the candidate disagrees with the current premise:
/// reload the current state and rebuild the candidate, never proceed with the
/// stale one. Deny/AskOwner variants arrive with the rules that produce them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionAuthorizationDecision {
    /// Allowed for exactly this use, under the carried evaluation id.
    AllowForThisUse(ActionPermissionEvaluationId),
    /// The candidate and the current premise disagree; reload and retry.
    NeedsRevalidation,
}

/// Tracks minted Action evaluation ids and enforces single use.
///
/// Holds the issued `RawId -> fingerprint` map only: a successful consume
/// removes the entry, so presence means unused and absence means unknown or
/// already consumed.
#[derive(Debug, Default)]
pub struct ActionEvaluationTracker {
    issued: HashMap<RawId, ActionEvalFingerprint>,
}

impl ActionEvaluationTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            issued: HashMap::new(),
        }
    }

    fn mint(&mut self, candidate: &ActionUseCandidate) -> ActionPermissionEvaluationId {
        let id = ActionPermissionEvaluationId(RawId::new());
        self.issued.insert(id.0, candidate.fingerprint());
        id
    }

    /// Consumes an id iff it is known, unused, and bound to `candidate`.
    ///
    /// Returns `false` for unknown ids, replays, and fingerprint mismatches.
    /// Only a matching presentation burns the id; a mismatch leaves the entry
    /// so the caller can retry with the correct candidate.
    pub fn consume(
        &mut self,
        id: &ActionPermissionEvaluationId,
        candidate: &ActionUseCandidate,
    ) -> bool {
        match self.issued.get(&id.0) {
            Some(bound) if *bound == candidate.fingerprint() => {
                self.issued.remove(&id.0);
                true
            }
            _ => false,
        }
    }
}

/// Pure policy for one Action-use authorization query (K-B.1).
///
/// The explicit Stage 4 arm is the delegated workspace capability with
/// `List / Read / Create / Edit`. A candidate whose correlation disagrees with
/// the freshly re-read current premise answers [`ActionAuthorizationDecision::NeedsRevalidation`];
/// a matching candidate earns a fresh single-use evaluation id. There is no
/// default-allow path: widening the world means adding an explicit arm here,
/// together with the rule/deny/ask producer that justifies it.
#[must_use]
pub fn authorize_action_use(
    candidate: &ActionUseCandidate,
    current: &CurrentActionPremise,
    tracker: &mut ActionEvaluationTracker,
) -> ActionAuthorizationDecision {
    let premise_matches = candidate.delegation == current.delegation
        && candidate.task == current.task
        && candidate.task_revision == current.task_revision
        && candidate.workspace == current.workspace;
    if !premise_matches {
        return ActionAuthorizationDecision::NeedsRevalidation;
    }
    // The explicit operation arm: every ActionKind variant is currently
    // allowed, and widening the vocabulary forces an edit here rather than a
    // silent pass-through.
    match candidate.operation {
        ActionKind::List | ActionKind::Read | ActionKind::Create | ActionKind::Edit => {}
    }
    ActionAuthorizationDecision::AllowForThisUse(tracker.mint(candidate))
}

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, RevisionInner};

    use super::{
        ActionAuthorizationDecision, ActionEvaluationTracker, ActionKind, ActionUseCandidate,
        CurrentActionPremise, authorize_action_use,
    };

    fn candidate() -> ActionUseCandidate {
        ActionUseCandidate {
            delegation: RawId::new(),
            task: RawId::new(),
            task_revision: RevisionInner::from_u64(2),
            workspace: RawId::new(),
            operation: ActionKind::Create,
            resolved_target: String::from("/srv/workspace/report.md"),
        }
    }

    fn premise(candidate: &ActionUseCandidate) -> CurrentActionPremise {
        CurrentActionPremise {
            delegation: candidate.delegation,
            task: candidate.task,
            task_revision: candidate.task_revision,
            workspace: candidate.workspace,
        }
    }

    #[test]
    fn action_kind_names_round_trip_as_a_closed_world() {
        for kind in [
            ActionKind::List,
            ActionKind::Read,
            ActionKind::Create,
            ActionKind::Edit,
        ] {
            assert_eq!(ActionKind::from_name(kind.as_str()), Some(kind));
        }
        assert_eq!(ActionKind::from_name("delete"), None);
        assert_eq!(ActionKind::from_name("execute"), None);
        assert_eq!(ActionKind::from_name(""), None);
    }

    #[test]
    fn a_matching_candidate_is_allowed_for_one_use() {
        let mut tracker = ActionEvaluationTracker::new();
        let candidate = candidate();
        let decision = authorize_action_use(&candidate, &premise(&candidate), &mut tracker);
        let ActionAuthorizationDecision::AllowForThisUse(evaluation) = decision else {
            panic!("a matching candidate is allowed: {decision:?}");
        };
        assert!(tracker.consume(&evaluation, &candidate));
        assert!(
            !tracker.consume(&evaluation, &candidate),
            "an evaluation id is single-use"
        );
    }

    #[test]
    fn a_candidate_that_disagrees_with_the_current_premise_needs_revalidation() {
        let mut tracker = ActionEvaluationTracker::new();
        let candidate = candidate();
        for current in [
            CurrentActionPremise {
                task_revision: RevisionInner::from_u64(3),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                workspace: RawId::new(),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                delegation: RawId::new(),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                task: RawId::new(),
                ..premise(&candidate)
            },
        ] {
            assert_eq!(
                authorize_action_use(&candidate, &current, &mut tracker),
                ActionAuthorizationDecision::NeedsRevalidation,
                "a moved premise never mints an evaluation"
            );
        }
    }

    #[test]
    fn a_minted_evaluation_cannot_be_consumed_for_another_target_or_operation() {
        let mut tracker = ActionEvaluationTracker::new();
        let candidate = candidate();
        let ActionAuthorizationDecision::AllowForThisUse(evaluation) =
            authorize_action_use(&candidate, &premise(&candidate), &mut tracker)
        else {
            panic!("the matching candidate is allowed");
        };
        let mut other_target = candidate.clone();
        other_target.resolved_target = String::from("/srv/workspace/other.md");
        assert!(
            !tracker.consume(&evaluation, &other_target),
            "the evaluation is bound to the resolved target"
        );
        let mut other_operation = candidate.clone();
        other_operation.operation = ActionKind::Edit;
        assert!(
            !tracker.consume(&evaluation, &other_operation),
            "the evaluation is bound to the operation"
        );
        assert!(
            tracker.consume(&evaluation, &candidate),
            "the original binding still consumes"
        );
    }
}
