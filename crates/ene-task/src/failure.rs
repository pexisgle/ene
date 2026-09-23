use crate::delegation::DelegationId;
use crate::task::{TaskId, TaskProgress, TaskRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskFailureKind {
    ConfirmedUnachievable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskFailurePremise {
    pub task: TaskRef,
    pub delegation: Option<DelegationId>,
    pub kind: TaskFailureKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskFailureOutcome {
    FailedAs(TaskRef),
    AlreadyFailed {
        task: TaskId,
    },
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    StalePremise {
        current: TaskRef,
    },
    MissingTask {
        task: TaskId,
    },
    MissingDelegation {
        delegation: DelegationId,
    },
}
