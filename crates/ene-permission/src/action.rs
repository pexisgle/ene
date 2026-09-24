use std::collections::HashMap;

use ene_primitive::{RawId, RevisionInner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    List,
    Read,
    Create,
    Edit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionPermissionEvaluationId(RawId);

impl ActionPermissionEvaluationId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActionEvalFingerprint(RawId, RawId, RevisionInner, RawId, ActionKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentActionPremise {
    pub delegation: RawId,
    pub task: RawId,
    pub task_revision: RevisionInner,
    pub workspace: RawId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionAuthorizationDecision {
    AllowForThisUse(ActionPermissionEvaluationId),
    Deny(ActionDenyCode),
    NeedsRevalidation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionDenyCode {
    NotInAllowlist,
    PremiseMismatch,
}

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
        return ActionAuthorizationDecision::Deny(ActionDenyCode::PremiseMismatch);
    }
    let in_allowlist = matches!(
        candidate.operation,
        ActionKind::List | ActionKind::Read | ActionKind::Create | ActionKind::Edit
    );
    if !in_allowlist {
        return ActionAuthorizationDecision::Deny(ActionDenyCode::NotInAllowlist);
    }
    ActionAuthorizationDecision::AllowForThisUse(tracker.mint(candidate))
}

#[cfg(test)]
mod tests {
    use ene_primitive::{RawId, RevisionInner};

    use super::{
        ActionAuthorizationDecision, ActionDenyCode, ActionEvaluationTracker, ActionKind,
        ActionUseCandidate, CurrentActionPremise, authorize_action_use,
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
    fn action_authorization_binds_premise_target_and_operation() {
        for operation in [
            ActionKind::List,
            ActionKind::Read,
            ActionKind::Create,
            ActionKind::Edit,
        ] {
            let candidate = ActionUseCandidate {
                operation,
                ..candidate()
            };
            let mut tracker = ActionEvaluationTracker::new();
            let ActionAuthorizationDecision::AllowForThisUse(evaluation) =
                authorize_action_use(&candidate, &premise(&candidate), &mut tracker)
            else {
                panic!("an exact allowlisted operation must be authorized");
            };
            assert!(tracker.consume(&evaluation, &candidate));
        }

        let candidate = candidate();
        for current in [
            CurrentActionPremise {
                delegation: RawId::new(),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                task: RawId::new(),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                task_revision: RevisionInner::from_u64(3),
                ..premise(&candidate)
            },
            CurrentActionPremise {
                workspace: RawId::new(),
                ..premise(&candidate)
            },
        ] {
            let mut tracker = ActionEvaluationTracker::new();
            assert_eq!(
                authorize_action_use(&candidate, &current, &mut tracker),
                ActionAuthorizationDecision::Deny(ActionDenyCode::PremiseMismatch)
            );
        }

        let mut tracker = ActionEvaluationTracker::new();
        let ActionAuthorizationDecision::AllowForThisUse(evaluation) =
            authorize_action_use(&candidate, &premise(&candidate), &mut tracker)
        else {
            panic!("the exact premise must mint an evaluation");
        };
        for altered in [
            ActionUseCandidate {
                resolved_target: String::from("/srv/workspace/other.md"),
                ..candidate.clone()
            },
            ActionUseCandidate {
                operation: ActionKind::Edit,
                ..candidate.clone()
            },
        ] {
            assert!(!tracker.consume(&evaluation, &altered));
        }
        assert!(tracker.consume(&evaluation, &candidate));
    }
}
