//! Task Contract and verifying runner (#1198).
//!
//! Validates goal / success criteria / artifacts before a runner starts,
//! forces the verifying state before completion, confines artifacts to the
//! workspace, gates follow-ups that widen scope, blocks new side effects
//! after cancel, and marks interrupted tasks on restart.

use thiserror::Error;

/// Minimal contract that must be present before a task may enter the runner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskContract {
    pub goal: String,
    pub success_criteria: Vec<String>,
    pub artifacts: Vec<String>,
    pub workspace: String,
    pub allowed_tools: Vec<String>,
}

impl TaskContract {
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.goal.trim().is_empty() {
            return Err(TaskError::IncompleteContract("goal is empty".into()));
        }
        if self.success_criteria.is_empty() {
            return Err(TaskError::IncompleteContract(
                "success_criteria is empty".into(),
            ));
        }
        if self.artifacts.is_empty() {
            return Err(TaskError::IncompleteContract("artifacts is empty".into()));
        }
        if self.workspace.trim().is_empty() {
            return Err(TaskError::IncompleteContract("workspace is empty".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    Running,
    Verifying,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub contract: TaskContract,
    pub state: TaskState,
    pub mailbox_revision: u64,
    pub artifacts: Vec<ArtifactRef>,
}

impl Task {
    pub fn new(id: impl Into<String>, contract: TaskContract) -> Result<Self, TaskError> {
        contract.validate()?;
        Ok(Self {
            id: id.into(),
            contract,
            state: TaskState::Pending,
            mailbox_revision: 0,
            artifacts: Vec::new(),
        })
    }

    pub fn start(&mut self) -> Result<(), TaskError> {
        match self.state {
            TaskState::Pending => {
                self.state = TaskState::Running;
                Ok(())
            }
            other => Err(TaskError::IllegalTransition {
                from: other,
                to: TaskState::Running,
            }),
        }
    }

    pub fn begin_verifying(&mut self) -> Result<(), TaskError> {
        match self.state {
            TaskState::Running => {
                self.state = TaskState::Verifying;
                Ok(())
            }
            other => Err(TaskError::IllegalTransition {
                from: other,
                to: TaskState::Verifying,
            }),
        }
    }

    pub fn complete(&mut self) -> Result<(), TaskError> {
        match self.state {
            TaskState::Verifying => {
                if self.artifacts.is_empty() || self.contract.success_criteria.is_empty() {
                    return Err(TaskError::VerificationFailed(
                        "artifacts or success_criteria empty".into(),
                    ));
                }
                // All artifacts must be workspace-confined.
                for art in &self.artifacts {
                    verify_artifact(art, self)?;
                }
                self.state = TaskState::Completed;
                Ok(())
            }
            TaskState::Running => Err(TaskError::VerificationFailed(
                "must go through verifying (model done alone is not enough)".into(),
            )),
            other => Err(TaskError::IllegalTransition {
                from: other,
                to: TaskState::Completed,
            }),
        }
    }

    pub fn register_artifact(&mut self, artifact: ArtifactRef) -> Result<(), TaskError> {
        if self.state == TaskState::Cancelled {
            return Err(TaskError::Cancelled);
        }
        verify_artifact(&artifact, self)?;
        self.artifacts.push(artifact);
        Ok(())
    }

    pub fn cancel(&mut self) {
        if matches!(
            self.state,
            TaskState::Pending | TaskState::Running | TaskState::Verifying
        ) {
            self.state = TaskState::Cancelled;
        }
    }

    pub fn start_side_effect(&self) -> Result<(), TaskError> {
        if self.state == TaskState::Cancelled {
            return Err(TaskError::Cancelled);
        }
        if self.state == TaskState::Interrupted {
            return Err(TaskError::Interrupted);
        }
        Ok(())
    }

    pub fn mark_interrupted_on_restart(&mut self) {
        if matches!(self.state, TaskState::Running | TaskState::Verifying) {
            self.state = TaskState::Interrupted;
        }
    }

    pub fn follow_up_requires_reapproval(&self, expanded_tools: &[String]) -> bool {
        expanded_tools
            .iter()
            .any(|t| !self.contract.allowed_tools.contains(t))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    pub path: String,
    pub workspace: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TaskError {
    #[error("incomplete contract: {0}")]
    IncompleteContract(String),
    #[error("workspace violation: {0}")]
    WorkspaceViolation(String),
    #[error("cancelled")]
    Cancelled,
    #[error("interrupted")]
    Interrupted,
    #[error("verification failed: {0}")]
    VerificationFailed(String),
    #[error("illegal transition {from:?} -> {to:?}")]
    IllegalTransition { from: TaskState, to: TaskState },
}

fn normalize_path(path: &str) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for comp in std::path::Path::new(path).components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

pub fn verify_artifact(artifact: &ArtifactRef, task: &Task) -> Result<(), TaskError> {
    let ws = normalize_path(&task.contract.workspace);
    let art = normalize_path(&artifact.path);
    if !art.starts_with(&ws) {
        return Err(TaskError::WorkspaceViolation(format!(
            "{} not in {}",
            artifact.path, task.contract.workspace
        )));
    }
    Ok(())
}

pub fn transition(task: &mut Task, next: TaskState) -> Result<(), TaskError> {
    use TaskState::{Cancelled, Completed, Failed, Interrupted, Pending, Running, Verifying};
    match (task.state, next) {
        (Running, Completed) => Err(TaskError::VerificationFailed(
            "must go through verifying".into(),
        )),
        (Cancelled, Running | Verifying | Completed) => Err(TaskError::Cancelled),
        (Pending, Running | Cancelled | Failed)
        | (Running, Verifying | Cancelled | Interrupted)
        | (Verifying, Completed | Failed | Cancelled | Interrupted) => {
            task.state = next;
            Ok(())
        }
        _ => Err(TaskError::IllegalTransition {
            from: task.state,
            to: next,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> TaskContract {
        TaskContract {
            goal: "research".into(),
            success_criteria: vec!["markdown exists".into()],
            artifacts: vec!["out.md".into()],
            workspace: "/tmp/ws".into(),
            allowed_tools: vec!["fs".into()],
        }
    }

    #[test]
    fn incomplete_contract_rejected() {
        let mut c = contract();
        c.goal = String::new();
        assert!(c.validate().is_err());
        let mut c2 = contract();
        c2.success_criteria.clear();
        assert!(c2.validate().is_err());
        let mut c3 = contract();
        c3.workspace = String::new();
        assert!(c3.validate().is_err());
        assert!(Task::new("t1", c).is_err());
    }

    #[test]
    fn model_done_without_verifying_rejected() {
        let mut task = Task::new("t1", contract()).expect("valid");
        task.start().expect("start");
        assert!(task.complete().is_err());
        task.begin_verifying().expect("verifying");
        // Still needs artifacts
        assert!(task.complete().is_err());
        task.register_artifact(ArtifactRef {
            path: "/tmp/ws/out.md".into(),
            workspace: "/tmp/ws".into(),
        })
        .expect("artifact");
        assert!(task.complete().is_ok());
        assert_eq!(task.state, TaskState::Completed);
    }

    #[test]
    fn workspace_violation_rejected() {
        let mut task = Task::new("t1", contract()).expect("valid");
        task.start().expect("start");
        task.begin_verifying().expect("verifying");
        let bad = ArtifactRef {
            path: "/etc/passwd".into(),
            workspace: "/tmp/ws".into(),
        };
        assert!(task.register_artifact(bad).is_err());
    }

    #[test]
    fn cancel_blocks_new_effects() {
        let mut task = Task::new("t1", contract()).expect("valid");
        task.start().expect("start");
        task.cancel();
        assert_eq!(task.state, TaskState::Cancelled);
        assert!(task.start_side_effect().is_err());
        assert!(
            task.register_artifact(ArtifactRef {
                path: "/tmp/ws/out.md".into(),
                workspace: "/tmp/ws".into(),
            })
            .is_err()
        );
        assert!(transition(&mut task, TaskState::Running).is_err());
    }

    #[test]
    fn interrupted_on_restart() {
        let mut task = Task::new("t1", contract()).expect("valid");
        task.start().expect("start");
        task.mark_interrupted_on_restart();
        assert_eq!(task.state, TaskState::Interrupted);
        assert!(task.start_side_effect().is_err());
        // Completed task stays completed through restart.
        let mut done = Task::new("t2", contract()).expect("valid");
        done.start().expect("start");
        done.begin_verifying().expect("verifying");
        done.register_artifact(ArtifactRef {
            path: "/tmp/ws/out.md".into(),
            workspace: "/tmp/ws".into(),
        })
        .expect("artifact");
        done.complete().expect("complete");
        done.mark_interrupted_on_restart();
        assert_eq!(done.state, TaskState::Completed);
    }

    #[test]
    fn scope_expansion_requires_reapproval() {
        let task = Task::new("t1", contract()).expect("valid");
        assert!(!task.follow_up_requires_reapproval(&["fs".to_owned()]));
        assert!(task.follow_up_requires_reapproval(&["fs".to_owned(), "exec".to_owned()]));
    }

    #[test]
    fn prefix_sibling_is_not_confinement() {
        let task = Task::new("t1", contract()).expect("valid");
        // /tmp/ws must not accept /tmp/ws2/out.md as confined.
        let sibling = ArtifactRef {
            path: "/tmp/ws2/out.md".into(),
            workspace: "/tmp/ws".into(),
        };
        assert!(verify_artifact(&sibling, &task).is_err());
        assert!(
            Task::new(
                "t1",
                TaskContract {
                    workspace: "/tmp/ws".into(),
                    ..contract()
                }
            )
            .expect("valid")
            .register_artifact(ArtifactRef {
                path: "/tmp/ws/out.md".into(),
                workspace: "/tmp/ws".into(),
            })
            .is_ok()
        );
    }

    #[test]
    fn parent_dir_escape_is_not_confinement() {
        let task = Task::new("t1", contract()).expect("valid");
        let escaped = ArtifactRef {
            path: "/tmp/ws/../etc/passwd".into(),
            workspace: "/tmp/ws".into(),
        };
        assert!(verify_artifact(&escaped, &task).is_err());
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        let mut pending = Task::new("t1", contract()).expect("valid");
        assert!(transition(&mut pending, TaskState::Completed).is_err());
        assert!(transition(&mut pending, TaskState::Verifying).is_err());
        let mut running = Task::new("r", contract()).expect("valid");
        running.start().expect("start");
        assert!(transition(&mut running, TaskState::Completed).is_err());
        let mut ver = Task::new("v", contract()).expect("valid");
        ver.start().expect("start");
        ver.begin_verifying().expect("verifying");
        assert!(transition(&mut ver, TaskState::Running).is_err());
        let mut done = Task::new("d", contract()).expect("valid");
        done.start().expect("start");
        done.begin_verifying().expect("verifying");
        done.register_artifact(ArtifactRef {
            path: "/tmp/ws/out.md".into(),
            workspace: "/tmp/ws".into(),
        })
        .expect("artifact");
        done.complete().expect("complete");
        assert!(transition(&mut done, TaskState::Running).is_err());
    }
}
