use std::collections::HashMap;

use ene_primitive::{RawId, RevisionInner};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    List,
    Read,
    Create,
    Edit,
}

impl ActionKind {
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
