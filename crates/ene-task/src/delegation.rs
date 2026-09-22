use ene_primitive::RawId;

use crate::task::{AssigneeRef, TaskId, TaskProgress, TaskRef};
use crate::workspace::{WorkspaceAssocId, WorkspaceFolderRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DelegationId(RawId);

impl DelegationId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskAgentEphemeralId(RawId);

impl TaskAgentEphemeralId {
    #[must_use]
    pub fn as_raw(self) -> RawId {
        self.0
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(RawId::new())
    }

    #[must_use]
    pub fn from_raw(raw: RawId) -> Self {
        Self(raw)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedWorkspace {
    pub assoc: WorkspaceAssocId,
    pub folder: WorkspaceFolderRef,
    pub save_target: Option<WorkspaceFolderRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationScope {
    pub workspace: Option<DelegatedWorkspace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationRef {
    pub delegation: DelegationId,
    pub task: TaskRef,
    pub delegator: AssigneeRef,
    pub agent: TaskAgentEphemeralId,
    pub scope: DelegationScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDelegationCommand {
    pub task: TaskRef,
    pub scope_copy: DelegationScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegationCreationPremise {
    pub delegation: DelegationId,
    pub task: TaskRef,
    pub agent: TaskAgentEphemeralId,
    pub scope_copy: DelegationScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationOutcome {
    Delegated(DelegationRef),
    /// The expected Task revision no longer matches; nothing was changed.
    StaleTaskRevision { current: TaskRef },
    /// The Task is terminal (`Completed` / `Failed` / `Cancelled`); no delegation is
    /// created and the revision is not advanced. Absorbing, so it is
    /// distinct from revision staleness.
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    MissingTask {
        task: TaskId,
    },
}
