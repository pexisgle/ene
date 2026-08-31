//! Task Contract and verifying runner.
//!
//! Validates goal / success criteria / artifacts before a runner starts,
//! forces the verifying state before completion, confines artifacts to the
//! workspace, gates follow-ups that widen scope, blocks new side effects
//! after cancel, and marks interrupted tasks on restart.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Success-criterion kinds the evaluator understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriterionKind {
    /// `path:<glob>` — at least one artifact path (relative to the workspace) matches.
    Path,
    /// `contains:<text>` — at least one artifact's contents contain the text. Bare text means the same.
    Contains,
    /// `size_min:<bytes>` — at least one artifact is at least that many bytes.
    SizeMin,
    /// `count_min:<n>` — at least that many artifacts are registered.
    CountMin,
}

/// One parsed success criterion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criterion {
    pub kind: CriterionKind,
    pub value: String,
}

/// Per-criterion evaluation result with concrete evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CriterionResult {
    pub criterion: String,
    pub satisfied: bool,
    pub evidence: String,
}

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
        validate_criteria(&self.success_criteria)?;
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
                let results = evaluate_criteria(
                    std::path::Path::new(&self.contract.workspace),
                    &self.contract.success_criteria,
                    &self.artifacts,
                );
                let unsatisfied: Vec<&CriterionResult> =
                    results.iter().filter(|result| !result.satisfied).collect();
                if !unsatisfied.is_empty() {
                    let detail = unsatisfied
                        .iter()
                        .map(|result| format!("'{}' - {}", result.criterion, result.evidence))
                        .collect::<Vec<_>>()
                        .join("; ");
                    return Err(TaskError::VerificationFailed(format!(
                        "success criteria not satisfied: {detail}"
                    )));
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

/// Resolve a raw artifact path against the job workspace: relative paths join
/// the workspace, absolute paths must stay inside it, and an existing file
/// must not escape through a symlink.
pub fn confine_artifact_path(workspace: &Path, raw: &str) -> Result<PathBuf, TaskError> {
    if raw.trim().is_empty() {
        return Err(TaskError::WorkspaceViolation("empty artifact path".into()));
    }
    let ws = workspace
        .canonicalize()
        .unwrap_or_else(|_| normalize_path(&workspace.to_string_lossy()));
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        ws.join(candidate)
    };
    let confined = normalize_path(&joined.to_string_lossy());
    if !confined.starts_with(&ws) {
        return Err(TaskError::WorkspaceViolation(format!(
            "{} not in {}",
            raw,
            workspace.display()
        )));
    }
    if let Ok(canonical) = confined.canonicalize()
        && !canonical.starts_with(&ws)
    {
        return Err(TaskError::WorkspaceViolation(format!(
            "{} escapes {} through a symlink",
            raw,
            workspace.display()
        )));
    }
    Ok(confined)
}

/// Reject unknown prefixed criterion kinds before a contract enters the runner.
pub fn validate_criteria(criteria: &[String]) -> Result<(), TaskError> {
    for raw in criteria {
        parse_criterion(raw).map_err(|err| match err {
            TaskError::IncompleteContract(msg) => {
                TaskError::IncompleteContract(format!("criterion {raw:?}: {msg}"))
            }
            other => other,
        })?;
    }
    Ok(())
}

fn parse_criterion(raw: &str) -> Result<Criterion, TaskError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(TaskError::IncompleteContract("empty criterion".into()));
    }
    if let Some((prefix, value)) = trimmed.split_once(':') {
        let value = value.trim();
        let kind = match prefix {
            "path" => CriterionKind::Path,
            "contains" => CriterionKind::Contains,
            "size_min" => CriterionKind::SizeMin,
            "count_min" => CriterionKind::CountMin,
            _ => {
                return Err(TaskError::IncompleteContract(format!(
                    "unknown criterion kind {prefix:?}"
                )));
            }
        };
        if value.is_empty() {
            return Err(TaskError::IncompleteContract(format!(
                "criterion kind {prefix:?} needs a value"
            )));
        }
        return Ok(Criterion {
            kind,
            value: value.to_owned(),
        });
    }
    Ok(Criterion {
        kind: CriterionKind::Contains,
        value: trimmed.to_owned(),
    })
}

/// Evaluate every criterion against the registered artifacts. Each result
/// carries concrete evidence; `satisfied` is true only when the evidence holds.
pub fn evaluate_criteria(
    workspace: &Path,
    criteria: &[String],
    artifacts: &[ArtifactRef],
) -> Vec<CriterionResult> {
    criteria
        .iter()
        .map(|raw| {
            let criterion = parse_criterion(raw).unwrap_or_else(|_| Criterion {
                kind: CriterionKind::Contains,
                value: raw.clone(),
            });
            evaluate_one(workspace, &criterion, raw, artifacts)
        })
        .collect()
}

fn evaluate_one(
    workspace: &Path,
    criterion: &Criterion,
    raw: &str,
    artifacts: &[ArtifactRef],
) -> CriterionResult {
    let ws = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    match criterion.kind {
        CriterionKind::Path => {
            for art in artifacts {
                let Some(rel) = artifact_relative(&ws, &art.path) else {
                    continue;
                };
                if glob_match(&criterion.value, &rel) {
                    return satisfied(raw, format!("artifact '{rel}' matches {}", criterion.value));
                }
            }
            unsatisfied(raw, format!("no artifact matches {}", criterion.value))
        }
        CriterionKind::Contains => {
            for art in artifacts {
                let Some(rel) = artifact_relative(&ws, &art.path) else {
                    continue;
                };
                if let Ok(body) = read_head(Path::new(&art.path))
                    && body.contains(&criterion.value)
                {
                    return satisfied(
                        raw,
                        format!("artifact '{rel}' contains '{}'", criterion.value),
                    );
                }
            }
            unsatisfied(raw, format!("no artifact contains '{}'", criterion.value))
        }
        CriterionKind::SizeMin => {
            let Ok(min) = criterion.value.parse::<u64>() else {
                return unsatisfied(raw, format!("invalid size_min '{}'", criterion.value));
            };
            for art in artifacts {
                let size = std::fs::metadata(&art.path).ok().map(|meta| meta.len());
                if size.is_some_and(|bytes| bytes >= min) {
                    return satisfied(
                        raw,
                        format!(
                            "artifact '{}' has {} bytes >= {min}",
                            art.path,
                            size.unwrap_or(0)
                        ),
                    );
                }
            }
            unsatisfied(raw, format!("no artifact is at least {min} bytes"))
        }
        CriterionKind::CountMin => {
            let Ok(min) = criterion.value.parse::<usize>() else {
                return unsatisfied(raw, format!("invalid count_min '{}'", criterion.value));
            };
            if artifacts.len() >= min {
                satisfied(
                    raw,
                    format!("{} artifacts registered >= {min}", artifacts.len()),
                )
            } else {
                unsatisfied(
                    raw,
                    format!("{} artifacts registered < {min}", artifacts.len()),
                )
            }
        }
    }
}

fn artifact_relative(ws: &Path, path: &str) -> Option<String> {
    let normalized = normalize_path(path);
    normalized
        .strip_prefix(ws)
        .ok()
        .map(|rel| rel.to_string_lossy().into_owned())
}

fn satisfied(criterion: &str, evidence: String) -> CriterionResult {
    CriterionResult {
        criterion: criterion.to_owned(),
        satisfied: true,
        evidence,
    }
}

fn unsatisfied(criterion: &str, evidence: String) -> CriterionResult {
    CriterionResult {
        criterion: criterion.to_owned(),
        satisfied: false,
        evidence,
    }
}

/// Read at most 1 MiB of a UTF-8 file for `contains` evaluation.
fn read_head(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::with_capacity(64 * 1024);
    file.take(1024 * 1024).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Minimal glob: `*` matches any run, `?` matches one char.
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0_usize, 0_usize);
    let mut star: Option<usize> = None;
    let mut mark = 0_usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
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

    fn real_workspace() -> (tempfile::TempDir, TaskContract) {
        let dir = tempfile::TempDir::new().expect("tmp");
        let contract = TaskContract {
            goal: "research".into(),
            success_criteria: vec!["contains:quarterly".into()],
            artifacts: vec!["out.md".into()],
            workspace: dir.path().to_string_lossy().into_owned(),
            allowed_tools: vec!["fs".into()],
        };
        (dir, contract)
    }

    fn write_artifact(dir: &tempfile::TempDir, body: &str) -> String {
        let path = dir.path().join("out.md");
        std::fs::write(&path, body).expect("write");
        path.to_string_lossy().into_owned()
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
        let mut c4 = contract();
        c4.success_criteria = vec!["bogus:x".into()];
        let err = Task::new("t1", c4).expect_err("unknown kind");
        assert!(err.to_string().contains("unknown criterion kind"));
    }

    #[test]
    fn model_done_without_verifying_rejected() {
        let (dir, contract) = real_workspace();
        let artifact = write_artifact(&dir, "quarterly report");
        let mut task = Task::new("t1", contract).expect("valid");
        task.start().expect("start");
        assert!(task.complete().is_err());
        task.begin_verifying().expect("verifying");
        // Still needs artifacts
        assert!(task.complete().is_err());
        task.register_artifact(ArtifactRef {
            path: artifact.clone(),
            workspace: task.contract.workspace.clone(),
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
    fn unsatisfied_criteria_block_completion() {
        let (dir, contract) = real_workspace();
        let artifact = write_artifact(&dir, "alpha only");
        let mut task = Task::new("t1", contract).expect("valid");
        task.start().expect("start");
        task.begin_verifying().expect("verifying");
        task.register_artifact(ArtifactRef {
            path: artifact.clone(),
            workspace: task.contract.workspace.clone(),
        })
        .expect("artifact");
        let err = task.complete().expect_err("unsatisfied");
        assert!(err.to_string().contains("success criteria not satisfied"));
        std::fs::write(&artifact, "quarterly numbers").expect("update");
        assert!(task.complete().is_ok());
        assert_eq!(task.state, TaskState::Completed);
    }

    #[test]
    fn path_and_size_criteria_evaluate() {
        let (dir, mut contract) = real_workspace();
        contract.success_criteria = vec![
            "path:*.md".into(),
            "size_min:5".into(),
            "count_min:1".into(),
        ];
        let artifact = write_artifact(&dir, "quarterly report");
        let mut task = Task::new("t1", contract).expect("valid");
        task.start().expect("start");
        task.begin_verifying().expect("verifying");
        task.register_artifact(ArtifactRef {
            path: artifact.clone(),
            workspace: task.contract.workspace.clone(),
        })
        .expect("artifact");
        assert!(task.complete().is_ok());
        let (dir2, mut contract2) = real_workspace();
        contract2.success_criteria = vec!["path:*.txt".into()];
        std::fs::write(dir2.path().join("out.md"), "quarterly report").expect("write");
        let artifact2 = dir2.path().join("out.md").to_string_lossy().into_owned();
        let mut task2 = Task::new("t2", contract2).expect("valid");
        task2.start().expect("start");
        task2.begin_verifying().expect("verifying");
        task2
            .register_artifact(ArtifactRef {
                path: artifact2.clone(),
                workspace: task2.contract.workspace.clone(),
            })
            .expect("artifact");
        assert!(task2.complete().is_err());
    }

    #[test]
    fn confine_artifact_path_confines_to_workspace() {
        let (dir_a, contract_a) = real_workspace();
        let (_dir_b, contract_b) = real_workspace();
        let ws_a = std::path::Path::new(&contract_a.workspace);
        let ws_b = std::path::Path::new(&contract_b.workspace);
        assert!(confine_artifact_path(ws_a, &ws_b.join("out.md").to_string_lossy()).is_err());
        assert!(confine_artifact_path(ws_a, "../outside.md").is_err());
        let ok = confine_artifact_path(ws_a, "out.md").expect("inside");
        assert_eq!(ok, ws_a.join("out.md"));
        let _ = dir_a;
    }

    #[test]
    fn glob_matching() {
        assert!(glob_match("*.md", "report.md"));
        assert!(glob_match("out/*.md", "out/a.md"));
        assert!(!glob_match("*.md", "report.txt"));
        assert!(glob_match("?eport.md", "report.md"));
        assert!(!glob_match("?eport.md", "report.txt"));
        assert!(glob_match("*", "any/path.md"));
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
        let (dir, contract) = real_workspace();
        write_artifact(&dir, "quarterly report");
        let mut task = Task::new("t1", contract).expect("valid");
        task.start().expect("start");
        task.mark_interrupted_on_restart();
        assert_eq!(task.state, TaskState::Interrupted);
        assert!(task.start_side_effect().is_err());
        // Completed task stays completed through restart.
        let (dir2, contract2) = real_workspace();
        let artifact = write_artifact(&dir2, "quarterly report");
        let mut done = Task::new("t2", contract2).expect("valid");
        done.start().expect("start");
        done.begin_verifying().expect("verifying");
        done.register_artifact(ArtifactRef {
            path: artifact.clone(),
            workspace: done.contract.workspace.clone(),
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
        let (dir, contract) = real_workspace();
        write_artifact(&dir, "quarterly report");
        let mut done = Task::new("d", contract).expect("valid");
        done.start().expect("start");
        done.begin_verifying().expect("verifying");
        done.register_artifact(ArtifactRef {
            path: dir.path().join("out.md").to_string_lossy().into_owned(),
            workspace: done.contract.workspace.clone(),
        })
        .expect("artifact");
        done.complete().expect("complete");
        assert!(transition(&mut done, TaskState::Running).is_err());
    }
}
