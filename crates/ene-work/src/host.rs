use crate::error::WorkError;
use crate::store::WorkStore;
use crate::types::{CompanionReport, DelegationMode, Job, JobStatus, NewJob, UpgradeReason};
use chrono::{DateTime, Utc};
use ene_registry::{Layer, ToolDefinition, ToolRegistry};
use ene_session::{DelegationId, SoulId};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Host for public/internal delegations. Auto-upgrade uses the same entity.
pub struct DelegationHost {
    store: Arc<WorkStore>,
    data_dir: PathBuf,
    max_active: u32,
    max_depth: u32,
}

impl DelegationHost {
    #[must_use]
    pub fn new(store: Arc<WorkStore>, data_dir: PathBuf) -> Self {
        Self::with_limits(store, data_dir, 8, 3)
    }

    #[must_use]
    pub fn with_limits(
        store: Arc<WorkStore>,
        data_dir: PathBuf,
        max_active: u32,
        max_depth: u32,
    ) -> Self {
        Self {
            store,
            data_dir,
            max_active,
            max_depth,
        }
    }

    #[must_use]
    pub fn store(&self) -> Arc<WorkStore> {
        Arc::clone(&self.store)
    }

    #[must_use]
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    pub fn start(&self, request: StartDelegation) -> Result<Job, WorkError> {
        let StartDelegation {
            soul_id,
            goal,
            mode,
            title,
            brief,
            plan,
            created_from_turn,
            depth,
        } = request;
        if depth >= self.max_depth {
            return Err(WorkError::DepthExceeded);
        }
        if mode == DelegationMode::Public && self.store.count_active(soul_id)? >= self.max_active {
            return Err(WorkError::SlotsFull);
        }
        let workspace = self
            .data_dir
            .join("workspaces")
            .join(soul_id.to_string())
            .join("jobs");
        std::fs::create_dir_all(&workspace)?;
        let job_id = DelegationId::new();
        let dir = workspace.join(job_id.to_string());
        std::fs::create_dir_all(&dir)?;
        let title = title.unwrap_or_else(|| truncate(&goal, 48));
        let workspace_dir = dir.to_string_lossy().into_owned();
        let job = if mode == DelegationMode::Public {
            let job = self.store.insert_job(&NewJob {
                id: Some(job_id),
                soul_id,
                title,
                goal: goal.clone(),
                mode,
                workspace_dir,
                created_from_turn,
                plan,
                brief,
            })?;
            self.store.set_status(job.id, JobStatus::Queued, None)?;
            self.store
                .get_job(job.id)?
                .ok_or_else(|| WorkError::UnknownJob(job.id.to_string()))?
        } else {
            Job {
                id: job_id,
                soul_id,
                title,
                goal: goal.clone(),
                mode,
                status: JobStatus::Created,
                progress_fraction: None,
                progress_note: None,
                workspace_dir,
                error_class: None,
                created_from_turn,
                plan,
                brief,
                created_at: Utc::now().to_rfc3339(),
                ended_at: None,
            }
        };
        self.store
            .mailbox_push(job.id, "parent_to_child", "task", &job.goal)?;
        Ok(job)
    }

    /// Surface requested a side-effect tool or blew the step budget. Do not run the tool.
    pub fn auto_upgrade(&self, request: UpgradeRequest) -> Result<Job, WorkError> {
        let brief = request.brief.unwrap_or_else(|| {
            format!(
                "surface stopped: {}. already learned: {}",
                request.reason.as_str(),
                request.steps_so_far
            )
        });
        self.start(StartDelegation {
            soul_id: request.soul_id,
            goal: request.goal,
            mode: DelegationMode::Public,
            title: Some("task".into()),
            brief: Some(brief),
            plan: None,
            created_from_turn: request.created_from_turn,
            depth: 0,
        })
    }

    pub fn progress(
        &self,
        id: DelegationId,
        fraction: Option<f32>,
        note: &str,
    ) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        if self.store.get_job(id)?.is_some() {
            self.store.set_progress(id, fraction, Some(note))?;
        }
        self.store
            .mailbox_push(id, "child_to_parent", "progress", note)?;
        Ok(CompanionReport {
            speech: format!("still working: {note}"),
            inner_intent: Some("progress".into()),
        })
    }

    pub fn complete(&self, id: DelegationId, summary: &str) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        if let Some(job) = self.store.get_job(id)? {
            if matches!(
                job.status,
                JobStatus::Completed | JobStatus::Cancelled | JobStatus::Interrupted
            ) {
                return Err(WorkError::AlreadyCompleted);
            }
            self.store.set_status(id, JobStatus::Completed, None)?;
        }
        self.store
            .mailbox_push(id, "child_to_parent", "complete", summary)?;
        Ok(CompanionReport {
            speech: format!("done — {summary}"),
            inner_intent: Some("complete".into()),
        })
    }

    pub fn fail(&self, id: DelegationId, summary: &str) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        if self.store.get_job(id)?.is_some() {
            self.store
                .set_status(id, JobStatus::Failed, Some("failed"))?;
        }
        self.store
            .mailbox_push(id, "child_to_parent", "failed", summary)?;
        Ok(CompanionReport {
            speech: format!("the task failed: {summary}"),
            inner_intent: Some("failed".into()),
        })
    }

    pub fn cancel(&self, id: DelegationId) -> Result<JobStatus, WorkError> {
        self.require_known(id)?;
        let Some(job) = self.store.get_job(id)? else {
            self.store
                .mailbox_push(id, "parent_to_child", "cancel", "")?;
            return Ok(JobStatus::Cancelled);
        };
        match job.status {
            JobStatus::Completed => Err(WorkError::AlreadyCompleted),
            JobStatus::Cancelled => Err(WorkError::Cancelled),
            _ => {
                self.store.set_status(id, JobStatus::Cancelled, None)?;
                Ok(JobStatus::Cancelled)
            }
        }
    }

    pub fn recover_interrupted(&self) -> Result<Vec<CompanionReport>, WorkError> {
        let jobs = self.store.interrupt_running()?;
        Ok(jobs
            .into_iter()
            .map(|job| {
                let speech = match job.status {
                    JobStatus::Queued | JobStatus::Created => format!(
                        "the task '{}' was waiting and was not started. want me to start it?",
                        job.title
                    ),
                    _ => format!(
                        "the task '{}' stopped in the middle. want me to start a new one?",
                        job.title
                    ),
                };
                CompanionReport {
                    speech,
                    inner_intent: Some("interrupted".into()),
                }
            })
            .collect())
    }

    pub fn question(&self, id: DelegationId, prompt: &str) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        self.store
            .mailbox_push(id, "child_to_parent", "question", prompt)?;
        Ok(CompanionReport {
            speech: prompt.to_owned(),
            inner_intent: Some("ask_user".into()),
        })
    }

    pub fn answer(&self, id: DelegationId, answer: &str) -> Result<(), WorkError> {
        self.require_known(id)?;
        self.store
            .mailbox_push(id, "parent_to_child", "answer", answer)
    }

    pub fn instruct(&self, id: DelegationId, message: &str) -> Result<(), WorkError> {
        self.require_known(id)?;
        self.store
            .mailbox_push(id, "parent_to_child", "task", message)
    }

    pub fn message(&self, id: DelegationId, message: &str) -> Result<(), WorkError> {
        self.require_known(id)?;
        self.store
            .mailbox_push(id, "parent_to_child", "message", message)
    }

    pub fn status_snapshot(&self, id: DelegationId) -> Result<Job, WorkError> {
        self.store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))
    }

    fn require_known(&self, id: DelegationId) -> Result<(), WorkError> {
        if self.store.get_job(id)?.is_some() {
            return Ok(());
        }
        if self.store.mailbox(id)?.is_empty() {
            return Err(WorkError::UnknownJob(id.to_string()));
        }
        Ok(())
    }
}

pub struct StartDelegation {
    pub soul_id: SoulId,
    pub goal: String,
    pub mode: DelegationMode,
    pub title: Option<String>,
    pub brief: Option<String>,
    pub plan: Option<String>,
    pub created_from_turn: Option<String>,
    pub depth: u32,
}

pub struct UpgradeRequest {
    pub soul_id: SoulId,
    pub goal: String,
    pub reason: UpgradeReason,
    pub steps_so_far: String,
    pub brief: Option<String>,
    pub created_from_turn: Option<String>,
}

/// Static check used by the surface router. Never executes the tool.
#[must_use]
pub fn surface_call_kind(registry: &ToolRegistry, name: &str) -> SurfaceCallKind {
    match registry.get(name) {
        None => SurfaceCallKind::Unknown,
        Some(def) if def.surface_visible() => SurfaceCallKind::Run,
        Some(_) => SurfaceCallKind::Upgrade,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceCallKind {
    Run,
    Upgrade,
    Unknown,
}

#[must_use]
pub fn should_upgrade_steps(step_index: u32, max_steps: u32) -> bool {
    step_index + 1 >= max_steps.max(1)
}

#[must_use]
pub fn fold_brief(steps: &[String], tool: Option<&str>) -> String {
    let mut out = String::from("so far: ");
    if steps.is_empty() {
        out.push_str("(nothing yet)");
    } else {
        out.push_str(&steps.join("; "));
    }
    if let Some(tool) = tool {
        out.push_str(". next tool requested: ");
        out.push_str(tool);
    }
    out
}

/// Unanswered child question older than `timeout` is assumed answered so the child can continue.
#[must_use]
pub fn question_timed_out(asked_at: DateTime<Utc>, now: DateTime<Utc>, timeout: Duration) -> bool {
    let limit = chrono::Duration::from_std(timeout).unwrap_or_else(|_| chrono::Duration::hours(24));
    now.signed_duration_since(asked_at) >= limit
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

pub fn def_is_side_effect(def: &ToolDefinition) -> bool {
    !def.side_effects.is_empty() && !def.name.starts_with("delegate.")
}

#[must_use]
pub fn layer_for_call(kind: SurfaceCallKind) -> Layer {
    match kind {
        SurfaceCallKind::Run => Layer::Surface,
        SurfaceCallKind::Upgrade | SurfaceCallKind::Unknown => Layer::Job,
    }
}
