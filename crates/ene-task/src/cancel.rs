use crate::task::{TaskId, TaskProgress};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelTaskCommand {
    pub task: TaskId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCancelOutcome {
    CancelAccepted,
    AlreadyCancelled,
    Superseded,
    TaskTerminal {
        task: TaskId,
        progress: TaskProgress,
    },
    MissingTask {
        task: TaskId,
    },
}
