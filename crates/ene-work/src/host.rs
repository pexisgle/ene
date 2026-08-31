use crate::error::WorkError;
use crate::questions::{combine_questions, route_combined_answers};
use crate::speech_gate::SpeechGate;
use crate::spill::{DEFAULT_SOFT_LIMIT_BYTES, bound_brief};
use crate::store::WorkStore;
use crate::task::{ArtifactRef, TaskContract, TaskError, TaskState};
use crate::types::{
    Artifact, CombinedQuestionTurn, CompanionReport, DelegationMode, Job, JobStatus, NewJob,
    NewToolExecution, OpenQuestion, ToolExecStatus, ToolExecution, UpgradeReason,
    WorkDelegationSettings,
};
use chrono::{DateTime, Utc};
use ene_registry::{Layer, ToolDefinition, ToolRegistry};
use ene_session::{DelegationId, QuestionId, SoulId};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Tool-confine and job-artifact root (`<data>/workspace`), kept off secrets.
#[must_use]
pub fn workspace_root(data_dir: &Path) -> PathBuf {
    data_dir.join("workspace")
}

/// Delivered copies for one soul: `<data>/workspace/jobs/<soul_id>/artifacts`.
#[must_use]
pub fn soul_artifacts_dir(data_dir: &Path, soul: SoulId) -> PathBuf {
    workspace_root(data_dir)
        .join("jobs")
        .join(soul.to_string())
        .join("artifacts")
}

pub(crate) fn sanitize_filename(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !out.ends_with('_') {
            out.push('_');
        }
    }
    if out.is_empty() {
        "artifact".to_owned()
    } else {
        out
    }
}

fn delivered_file_name(id: &str, title: &str, src: &Path) -> String {
    let stem = sanitize_filename(title);
    match src.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => format!("{id}_{stem}.{ext}"),
        _ => format!("{id}_{stem}"),
    }
}

/// Host for public/internal delegations. Auto-upgrade uses the same entity.
pub struct DelegationHost {
    store: Arc<WorkStore>,
    data_dir: PathBuf,
    settings: WorkDelegationSettings,
    speech_gate: Arc<SpeechGate>,
    question_gate: Mutex<()>,
    report_tx: Mutex<Option<mpsc::UnboundedSender<CompanionReport>>>,
    job_wake: Mutex<Option<mpsc::UnboundedSender<Job>>>,
}

impl DelegationHost {
    #[must_use]
    pub fn new(store: Arc<WorkStore>, data_dir: PathBuf) -> Self {
        Self::with_settings(store, data_dir, WorkDelegationSettings::default())
    }

    #[must_use]
    pub fn with_limits(
        store: Arc<WorkStore>,
        data_dir: PathBuf,
        max_active: u32,
        max_depth: u32,
    ) -> Self {
        Self::with_settings(
            store,
            data_dir,
            WorkDelegationSettings {
                max_active,
                max_depth,
                question_timeout_hours: 24,
            },
        )
    }

    #[must_use]
    pub fn with_settings(
        store: Arc<WorkStore>,
        data_dir: PathBuf,
        settings: WorkDelegationSettings,
    ) -> Self {
        Self {
            store,
            data_dir,
            settings,
            speech_gate: Arc::new(SpeechGate::new()),
            question_gate: Mutex::new(()),
            report_tx: Mutex::new(None),
            job_wake: Mutex::new(None),
        }
    }

    /// Deliver companion speech to the HTTP live bus and session log.
    pub fn set_report_sink(&self, tx: mpsc::UnboundedSender<CompanionReport>) {
        *self.report_tx.lock() = Some(tx);
    }

    /// Close the live-bus channel so the HTTP report task can exit.
    pub fn clear_report_sink(&self) {
        self.report_tx.lock().take();
    }

    /// Wake the job runner when a delegation is accepted. Missing receiver is a no-op.
    pub fn set_job_wake(&self, tx: mpsc::UnboundedSender<Job>) {
        *self.job_wake.lock() = Some(tx);
    }

    /// Drop the runner channel so serve shutdown can exit.
    pub fn clear_job_wake(&self) {
        self.job_wake.lock().take();
    }

    fn wake_job(&self, job: &Job) {
        if let Some(tx) = self.job_wake.lock().as_ref() {
            drop(tx.send(job.clone()));
        }
    }

    #[must_use]
    pub fn settings(&self) -> WorkDelegationSettings {
        self.settings
    }

    #[must_use]
    pub fn speech_gate(&self) -> Arc<SpeechGate> {
        Arc::clone(&self.speech_gate)
    }

    #[must_use]
    pub fn store(&self) -> Arc<WorkStore> {
        Arc::clone(&self.store)
    }

    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Queue or release job-completion speech according to the voice gap.
    pub fn mark_user_speaking(&self, speaking: bool) -> Vec<CompanionReport> {
        self.speech_gate.set_user_speaking(speaking);
        if speaking {
            Vec::new()
        } else {
            let drained = self.speech_gate.drain_when_gap();
            for report in &drained {
                self.publish_report(report);
            }
            drained
        }
    }

    fn soul_id_of(&self, id: DelegationId) -> Result<SoulId, WorkError> {
        Ok(self
            .store
            .get_job(id)?
            .map_or_else(|| SoulId::from_uuid(Uuid::nil()), |job| job.soul_id))
    }

    fn publish_report(&self, report: &CompanionReport) {
        if report.speech.is_empty() {
            return;
        }
        if let Some(tx) = self.report_tx.lock().as_ref() {
            drop(tx.send(report.clone()));
        }
    }

    /// Offer companion speech through the voice-gap gate and the live-bus sink.
    pub fn deliver_companion_report(&self, report: CompanionReport) -> CompanionReport {
        self.deliver_or_queue(report, "queued")
    }

    fn deliver_or_queue(&self, report: CompanionReport, queued_intent: &str) -> CompanionReport {
        let soul_id = report.soul_id;
        let job_id = report.job_id;
        if let Some(delivered) = self.speech_gate.offer(report) {
            self.publish_report(&delivered);
            delivered
        } else {
            CompanionReport {
                soul_id,
                job_id,
                speech: String::new(),
                inner_intent: Some(queued_intent.to_owned()),
                starts_conversation: false,
            }
        }
    }

    fn speak(
        soul_id: SoulId,
        job_id: Option<DelegationId>,
        speech: String,
        intent: &str,
        starts_conversation: bool,
    ) -> CompanionReport {
        CompanionReport {
            soul_id,
            job_id,
            speech,
            inner_intent: Some(intent.to_owned()),
            starts_conversation,
        }
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
            parent_id,
            success_criteria,
            allowed_tools,
        } = request;
        if !success_criteria.is_empty() {
            crate::task::validate_criteria(&success_criteria)
                .map_err(|err| WorkError::InvalidContract(err.to_string()))?;
        }
        let mode = self.enforce_secrecy(parent_id, mode)?;
        if depth >= self.settings.max_depth {
            return Err(WorkError::DepthExceeded);
        }
        if mode == DelegationMode::Public
            && self.store.count_active(soul_id)? >= self.settings.max_active
        {
            return Err(WorkError::SlotsFull);
        }
        let workspace = workspace_root(&self.data_dir)
            .join("jobs")
            .join(soul_id.to_string());
        std::fs::create_dir_all(&workspace)?;
        let job_id = DelegationId::new();
        let dir = workspace.join(job_id.to_string());
        std::fs::create_dir_all(&dir)?;
        let title = title.unwrap_or_else(|| truncate(&goal, 48));
        let workspace_dir = dir.to_string_lossy().into_owned();
        let brief = brief.map(|text| {
            bound_brief(
                &text,
                std::path::Path::new(&workspace_dir),
                DEFAULT_SOFT_LIMIT_BYTES,
            )
            .unwrap_or(text)
        });
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
                success_criteria,
                allowed_tools: allowed_tools.clone(),
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
                plan_approved: false,
                success_criteria,
                allowed_tools,
                pending_allowed_tools: None,
                created_at: Utc::now().to_rfc3339(),
                ended_at: None,
            }
        };
        self.store.record_meta(job.id, mode, depth)?;
        self.store
            .mailbox_push(job.id, "parent_to_child", "task", &job.goal)?;
        self.wake_job(&job);
        Ok(job)
    }

    fn enforce_secrecy(
        &self,
        parent_id: Option<DelegationId>,
        mode: DelegationMode,
    ) -> Result<DelegationMode, WorkError> {
        if let Some(parent_id) = parent_id {
            let parent_mode = self
                .store
                .delegation_mode(parent_id)?
                .ok_or_else(|| WorkError::UnknownJob(parent_id.to_string()))?;
            if parent_mode == DelegationMode::Internal && mode == DelegationMode::Public {
                return Err(WorkError::SecrecyViolation);
            }
            if parent_mode == DelegationMode::Internal {
                return Ok(DelegationMode::Internal);
            }
        }
        Ok(mode)
    }

    pub fn present_plan(&self, id: DelegationId, plan: &str) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        self.store.set_plan(id, plan)?;
        self.store
            .mailbox_push(id, "child_to_parent", "question", &format!("plan:\n{plan}"))?;
        Ok(Self::speak(
            self.soul_id_of(id)?,
            Some(id),
            format!("here's the plan: {plan}"),
            "ask_plan",
            true,
        ))
    }

    pub fn approve_plan(&self, id: DelegationId) -> Result<(), WorkError> {
        self.require_known(id)?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))?;
        if job.plan.as_ref().is_none_or(|plan| plan.trim().is_empty()) {
            return Err(WorkError::PlanNotApproved);
        }
        self.store.set_plan_approved(id)?;
        Ok(())
    }

    pub fn mutating_work_allowed(&self, id: DelegationId) -> Result<bool, WorkError> {
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))?;
        if job.plan.as_ref().is_none_or(|plan| plan.trim().is_empty()) {
            return Ok(false);
        }
        Ok(job.plan_approved)
    }

    pub fn require_mutating_allowed(&self, id: DelegationId) -> Result<(), WorkError> {
        if self.mutating_work_allowed(id)? {
            Ok(())
        } else {
            Err(WorkError::PlanNotApproved)
        }
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
            parent_id: None,
            success_criteria: Vec::new(),
            allowed_tools: Vec::new(),
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
        Ok(Self::speak(
            self.soul_id_of(id)?,
            Some(id),
            format!("still working: {note}"),
            "progress",
            false,
        ))
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
            if job.success_criteria.is_empty() {
                // Legacy delegation: completion needs no criteria, artifacts, or verifying state.
                self.verify_job_completion(&job)?;
                self.deliver_job_artifacts(id)?;
                self.store.set_status(id, JobStatus::Completed, None)?;
            } else {
                self.verify_job_completion(&job)?;
                if job.status != JobStatus::Verifying {
                    return Err(WorkError::VerificationFailed(
                        "must go through verifying (model done alone is not enough)".into(),
                    ));
                }
                self.deliver_job_artifacts(id)?;
                self.store.set_status(id, JobStatus::Completed, None)?;
            }
        }
        self.store
            .mailbox_push(id, "child_to_parent", "complete", summary)?;
        Ok(self.deliver_or_queue(
            Self::speak(
                self.soul_id_of(id)?,
                Some(id),
                format!("done — {summary}"),
                "complete",
                true,
            ),
            "complete_queued",
        ))
    }

    fn verify_job_completion(&self, job: &Job) -> Result<(), WorkError> {
        let artifacts = self.store.artifacts_for(job.id)?;
        if job.success_criteria.is_empty() {
            // Legacy job: only ensure registered artifacts stay in the job's own workspace.
            for art in &artifacts {
                crate::task::confine_artifact_path(
                    std::path::Path::new(&job.workspace_dir),
                    &art.path,
                )
                .map_err(|err| match err {
                    TaskError::WorkspaceViolation(msg) => WorkError::WorkspaceViolation(msg),
                    TaskError::VerificationFailed(msg) => WorkError::VerificationFailed(msg),
                    other => WorkError::WorkspaceViolation(other.to_string()),
                })?;
            }
            return Ok(());
        }
        if artifacts.is_empty() {
            return Err(WorkError::VerificationFailed(
                "model done alone is not enough: no artifacts registered for task contract".into(),
            ));
        }
        let contract = TaskContract {
            goal: job.goal.clone(),
            success_criteria: job.success_criteria.clone(),
            artifacts: artifacts.iter().map(|a| a.path.clone()).collect(),
            workspace: job.workspace_dir.clone(),
            allowed_tools: job.allowed_tools.clone(),
        };
        if let Err(err) = contract.validate() {
            return Err(WorkError::InvalidContract(err.to_string()));
        }
        let mut task = crate::task::Task {
            id: job.id.to_string(),
            contract,
            state: TaskState::Verifying,
            mailbox_revision: 0,
            artifacts: artifacts
                .iter()
                .map(|a| ArtifactRef {
                    path: a.path.clone(),
                    workspace: job.workspace_dir.clone(),
                })
                .collect(),
        };
        task.complete().map_err(|err| match err {
            TaskError::VerificationFailed(msg) => WorkError::VerificationFailed(msg),
            TaskError::WorkspaceViolation(msg) => WorkError::WorkspaceViolation(msg),
            other => WorkError::VerificationFailed(other.to_string()),
        })?;
        Ok(())
    }

    pub fn begin_verifying(&self, id: DelegationId) -> Result<(), WorkError> {
        self.require_known(id)?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))?;
        if job.status != JobStatus::Running {
            return Err(WorkError::VerificationFailed(format!(
                "cannot begin verifying from status {}",
                job.status.as_str()
            )));
        }
        self.store.set_status(id, JobStatus::Verifying, None)?;
        Ok(())
    }

    pub fn fail(&self, id: DelegationId, summary: &str) -> Result<CompanionReport, WorkError> {
        self.require_known(id)?;
        if self.store.get_job(id)?.is_some() {
            self.store
                .set_status(id, JobStatus::Failed, Some("failed"))?;
        }
        self.store
            .mailbox_push(id, "child_to_parent", "failed", summary)?;
        Ok(self.deliver_or_queue(
            Self::speak(
                self.soul_id_of(id)?,
                Some(id),
                format!("the task failed: {summary}"),
                "failed",
                true,
            ),
            "failed_queued",
        ))
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
                for exec in self.store.list_tool_executions_for_job(id)? {
                    if !exec.status.is_terminal() {
                        let _ = self.store.complete_tool_execution_once(
                            &exec.execution_id,
                            ToolExecStatus::Cancelled,
                            None,
                            None,
                        )?;
                    }
                }
                Ok(JobStatus::Cancelled)
            }
        }
    }

    pub fn recover_interrupted(&self) -> Result<Vec<CompanionReport>, WorkError> {
        let jobs = self.store.interrupt_running()?;
        let mut reports: Vec<CompanionReport> = jobs
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
                    soul_id: job.soul_id,
                    job_id: Some(job.id),
                    speech,
                    inner_intent: Some("interrupted".into()),
                    starts_conversation: true,
                }
            })
            .collect();
        reports.extend(self.recover_tool_executions()?);
        Ok(reports)
    }

    pub fn begin_tool_execution(&self, new: &NewToolExecution) -> Result<ToolExecution, WorkError> {
        self.store.insert_tool_execution(new)
    }

    pub fn tool_execution(&self, execution_id: &str) -> Result<Option<ToolExecution>, WorkError> {
        self.store.get_tool_execution(execution_id)
    }

    pub fn apply_tool_completion(
        &self,
        execution_id: &str,
        status: ToolExecStatus,
        error_class: Option<&str>,
        summary: &str,
    ) -> Result<Option<CompanionReport>, WorkError> {
        let Some(row) = self.store.get_tool_execution(execution_id)? else {
            return Err(WorkError::UnknownExecution(execution_id.to_owned()));
        };
        if !self.store.complete_tool_execution_once(
            execution_id,
            status,
            error_class,
            Some(summary),
        )? {
            return Ok(None);
        }
        if let Some(job_id) = row.job_id {
            self.store
                .mailbox_push(job_id, "child_to_parent", "tool_complete", summary)?;
        }
        let speech = match status {
            ToolExecStatus::Cancelled => format!("{} was cancelled", row.tool_name),
            ToolExecStatus::TimedOut => format!("{} timed out", row.tool_name),
            ToolExecStatus::PluginCrash => format!(
                "{} stopped ({})",
                row.tool_name,
                error_class.unwrap_or("plugin_crash")
            ),
            ToolExecStatus::Failed => format!("{} failed: {summary}", row.tool_name),
            _ => format!("{} finished: {summary}", row.tool_name),
        };
        let intent = match status {
            ToolExecStatus::Cancelled => "tool_cancelled",
            ToolExecStatus::TimedOut => "tool_timeout",
            ToolExecStatus::PluginCrash => "tool_plugin_crash",
            ToolExecStatus::Failed => "tool_failed",
            _ => "tool_complete",
        };
        Ok(Some(self.deliver_or_queue(
            Self::speak(row.soul_id, row.job_id, speech, intent, true),
            "tool_complete_queued",
        )))
    }

    pub fn cancel_tool_execution(&self, execution_id: &str) -> Result<String, WorkError> {
        let Some(row) = self.store.get_tool_execution(execution_id)? else {
            return Ok("unknown".to_owned());
        };
        if row.status.is_terminal() {
            return Ok("already_terminal".to_owned());
        }
        let _ =
            self.apply_tool_completion(execution_id, ToolExecStatus::Cancelled, None, "cancelled")?;
        Ok("cancelled".to_owned())
    }

    pub fn timeout_tool_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<CompanionReport>, WorkError> {
        self.apply_tool_completion(
            execution_id,
            ToolExecStatus::TimedOut,
            Some("timeout"),
            "timeout",
        )
    }

    pub fn crash_tool_execution(
        &self,
        execution_id: &str,
        error_class: &str,
    ) -> Result<Option<CompanionReport>, WorkError> {
        self.apply_tool_completion(
            execution_id,
            ToolExecStatus::PluginCrash,
            Some(error_class),
            error_class,
        )
    }

    pub fn recover_tool_executions(&self) -> Result<Vec<CompanionReport>, WorkError> {
        let running = self.store.list_running_tool_executions()?;
        let mut reports = Vec::new();
        for row in running {
            if let Some(report) = self.crash_tool_execution(&row.execution_id, "host_restart")? {
                reports.push(report);
            }
        }
        Ok(reports)
    }

    pub fn question(&self, id: DelegationId, prompt: &str) -> Result<CompanionReport, WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        self.store
            .mailbox_push(id, "child_to_parent", "question", prompt)?;
        let report = Self::speak(
            self.soul_id_of(id)?,
            Some(id),
            prompt.to_owned(),
            "ask_user",
            true,
        );
        self.publish_report(&report);
        Ok(report)
    }

    pub fn open_questions(&self, id: DelegationId) -> Result<Vec<OpenQuestion>, WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        self.store.open_questions(id)
    }

    pub fn combine_pending_questions(
        &self,
        id: DelegationId,
    ) -> Result<CombinedQuestionTurn, WorkError> {
        let questions = self.open_questions(id)?;
        Ok(combine_questions(&questions))
    }

    pub fn apply_combined_answers(
        &self,
        turn: &CombinedQuestionTurn,
        answers: &[String],
    ) -> Result<(), WorkError> {
        let Some(question) = turn.questions.first() else {
            if answers.is_empty() {
                return Ok(());
            }
            return Err(WorkError::QuestionAnswerCount {
                expected: 0,
                actual: answers.len(),
            });
        };
        if turn
            .questions
            .iter()
            .any(|pending| pending.delegation_id != question.delegation_id)
        {
            for (delegation_id, answer) in route_combined_answers(turn, answers) {
                self.answer(delegation_id, &answer)?;
            }
            return Ok(());
        }
        self.answer_pending(question.delegation_id, answers)?;
        Ok(())
    }

    pub fn answer(&self, id: DelegationId, answer: &str) -> Result<(), WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        let pending = self.store.open_questions(id)?;
        let Some(question) = pending.first() else {
            return Err(WorkError::NoOpenQuestion);
        };
        self.store
            .mailbox_push_for_question(id, question.mailbox_seq, "answer", answer)
    }

    /// Answer one pending question identified by its stable question id.
    pub fn answer_question(
        &self,
        id: DelegationId,
        question_id: QuestionId,
        answer: &str,
    ) -> Result<OpenQuestion, WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        let pending = self.store.open_questions(id)?;
        let Some(question) = pending
            .into_iter()
            .find(|question| question.question_id() == question_id)
        else {
            return Err(WorkError::QuestionAlreadyResolved);
        };
        self.store
            .mailbox_push_for_question(id, question.mailbox_seq, "answer", answer)?;
        Ok(question)
    }

    /// Answer every pending question on a job with the same text.
    pub fn answer_all_pending(
        &self,
        id: DelegationId,
        answer: &str,
    ) -> Result<Vec<OpenQuestion>, WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        let pending = self.store.open_questions(id)?;
        if pending.is_empty() {
            return Err(WorkError::QuestionAlreadyResolved);
        }
        for question in &pending {
            self.store
                .mailbox_push_for_question(id, question.mailbox_seq, "answer", answer)?;
        }
        Ok(pending)
    }

    /// Answer all pending questions in mailbox order.
    pub fn answer_pending(
        &self,
        id: DelegationId,
        answers: &[String],
    ) -> Result<Vec<OpenQuestion>, WorkError> {
        let _question_guard = self.question_gate.lock();
        self.require_known(id)?;
        let pending = self.store.open_questions(id)?;
        if pending.is_empty() {
            return Err(WorkError::QuestionAlreadyResolved);
        }
        if pending.len() != answers.len() {
            return Err(WorkError::QuestionAnswerCount {
                expected: pending.len(),
                actual: answers.len(),
            });
        }
        for (question, answer) in pending.iter().zip(answers) {
            self.store
                .mailbox_push_for_question(id, question.mailbox_seq, "answer", answer)?;
        }
        Ok(pending)
    }

    pub fn instruct(&self, id: DelegationId, message: &str) -> Result<(), WorkError> {
        self.require_known(id)?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))?;
        if job.status == JobStatus::Cancelled || job.status == JobStatus::Interrupted {
            return Err(WorkError::Cancelled);
        }
        if (message.to_ascii_lowercase().contains("allow:")
            || message.to_ascii_lowercase().contains("tool:"))
            && !job.allowed_tools.is_empty()
        {
            let expanded = Self::extract_tools(message);
            let new_tools: Vec<String> = expanded
                .into_iter()
                .filter(|tool| !job.allowed_tools.contains(tool))
                .collect();
            if !new_tools.is_empty() {
                let mut pending = job.pending_allowed_tools.unwrap_or_default();
                for tool in &new_tools {
                    if !pending.contains(tool) {
                        pending.push(tool.clone());
                    }
                }
                self.store.set_pending_allowed_tools(id, Some(&pending))?;
                return Err(WorkError::ScopeWideningPending { tools: new_tools });
            }
        }
        self.store
            .mailbox_push(id, "parent_to_child", "task", message)
    }

    /// Explicit approval event for a previously requested scope widening.
    pub fn approve_scope_widening(&self, id: DelegationId) -> Result<Vec<String>, WorkError> {
        self.require_known(id)?;
        let job = self
            .store
            .get_job(id)?
            .ok_or_else(|| WorkError::UnknownJob(id.to_string()))?;
        let Some(pending) = job.pending_allowed_tools else {
            return Err(WorkError::NoPendingScopeWidening);
        };
        let mut allowed = job.allowed_tools;
        for tool in &pending {
            if !allowed.contains(tool) {
                allowed.push(tool.clone());
            }
        }
        self.store.set_allowed_tools(id, &allowed)?;
        self.store.set_pending_allowed_tools(id, None)?;
        Ok(pending)
    }

    pub fn register_artifact_for_job(
        &self,
        job_id: DelegationId,
        artifact: Artifact,
    ) -> Result<Artifact, WorkError> {
        let job = self
            .store
            .get_job(job_id)?
            .ok_or_else(|| WorkError::UnknownJob(job_id.to_string()))?;
        if job.status == JobStatus::Cancelled {
            return Err(WorkError::Cancelled);
        }
        if job.status == JobStatus::Interrupted {
            return Err(WorkError::Interrupted);
        }
        let confined = crate::task::confine_artifact_path(
            std::path::Path::new(&job.workspace_dir),
            &artifact.path,
        )
        .map_err(|err| match err {
            TaskError::WorkspaceViolation(msg) => WorkError::WorkspaceViolation(msg),
            TaskError::VerificationFailed(msg) => WorkError::VerificationFailed(msg),
            other => WorkError::WorkspaceViolation(other.to_string()),
        })?;
        let path = confined.display().to_string();
        let mut stored = artifact;
        stored.path = path;
        stored.size_bytes = std::fs::metadata(&stored.path)
            .ok()
            .and_then(|meta| i64::try_from(meta.len()).ok());
        self.store.register_artifact(stored)
    }

    fn extract_tools(message: &str) -> Vec<String> {
        let mut out = Vec::new();
        for token in message.split(|c: char| c == ',' || c.is_whitespace()) {
            let t = token
                .trim()
                .trim_matches(|c| c == '"' || c == '\'' || c == ':' || c == ';');
            if !t.is_empty()
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
                && t.contains('.')
            {
                out.push(t.to_owned());
            }
        }
        out
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

    pub fn resolve_question_timeouts(
        &self,
        now: DateTime<Utc>,
        timeout: Option<Duration>,
    ) -> Result<Vec<CompanionReport>, WorkError> {
        let _question_guard = self.question_gate.lock();
        let timeout = timeout.unwrap_or_else(|| {
            Duration::from_secs(u64::from(self.settings.question_timeout_hours) * 3_600)
        });
        let jobs = self.store.list_jobs_all()?;
        let mut reports = Vec::new();
        for job in jobs {
            if !matches!(
                job.status,
                JobStatus::Created | JobStatus::Queued | JobStatus::Running
            ) {
                continue;
            }
            let pending = self.store.open_questions(job.id)?;
            if pending.is_empty()
                || pending.iter().any(|question| {
                    let asked_at = DateTime::parse_from_rfc3339(&question.asked_at)
                        .map_or(now, |ts| ts.with_timezone(&Utc));
                    !question_timed_out(asked_at, now, timeout)
                })
            {
                continue;
            }
            let prompts = pending
                .iter()
                .map(|question| truncate(&question.prompt, 48))
                .collect::<Vec<_>>()
                .join("; ");
            let assumption =
                format!("no answer after timeout — proceeding with best guess for: {prompts}");
            self.store
                .mailbox_push(job.id, "parent_to_child", "assumption", &assumption)?;
            reports.push(self.progress(job.id, None, &assumption)?);
        }
        Ok(reports)
    }

    fn deliver_job_artifacts(&self, job_id: DelegationId) -> Result<(), WorkError> {
        for art in self.store.artifacts_for(job_id)? {
            if art.delivered {
                continue;
            }
            let path = self.place_delivered_artifact(&art)?;
            self.store.mark_delivered(&art.id, &path)?;
        }
        Ok(())
    }

    fn place_delivered_artifact(&self, art: &Artifact) -> Result<String, WorkError> {
        let dest_dir = soul_artifacts_dir(&self.data_dir, art.soul_id);
        std::fs::create_dir_all(&dest_dir)?;
        let src = Path::new(&art.path);
        if !src.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("artifact source is not a file: {}", art.path),
            )
            .into());
        }
        let dest = dest_dir.join(delivered_file_name(&art.id, &art.title, src));
        if src == dest.as_path() {
            return Ok(art.path.clone());
        }
        std::fs::copy(src, &dest)?;
        Ok(dest.to_string_lossy().into_owned())
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
    pub parent_id: Option<DelegationId>,
    pub success_criteria: Vec<String>,
    pub allowed_tools: Vec<String>,
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
