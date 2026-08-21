use crate::error::WorkError;
use crate::host::DelegationHost;
use crate::router::JobLayerRouter;
use crate::types::{Job, JobStatus};
use ene_kernel::{
    ConversationModel, HarnessSettings, LaneHandle, LaneOptions, MindSettings, SurfaceRouter,
};
use ene_registry::ToolRegistry;
use ene_session::{
    NewSession, ProjectOptions, Role, SessionCreatedBy, SessionKind, SessionStore, derive_messages,
};
use std::sync::Arc;
use std::time::Duration;

/// Inputs for one back-harness job-lane run.
pub struct JobDrive {
    pub host: Arc<DelegationHost>,
    pub registry: Arc<ToolRegistry>,
    pub sessions: Arc<SessionStore>,
    pub model: Arc<dyn ConversationModel>,
    pub job: Job,
    pub step_budget: u32,
    pub wall: Duration,
}

/// Open a job session, run the model with job-layer tools, then complete or fail.
pub async fn drive_job(drive: JobDrive) -> Result<(), WorkError> {
    let JobDrive {
        host,
        registry,
        sessions,
        model,
        job,
        step_budget,
        wall,
    } = drive;
    match host.store().get_job(job.id)? {
        Some(current)
            if matches!(
                current.status,
                JobStatus::Cancelled
                    | JobStatus::Completed
                    | JobStatus::Failed
                    | JobStatus::Interrupted
            ) =>
        {
            return Ok(());
        }
        Some(_) => {
            host.store().set_status(job.id, JobStatus::Running, None)?;
        }
        None => {}
    }
    let session = sessions
        .create_session(NewSession {
            soul_id: job.soul_id,
            body_id: None,
            kind: SessionKind::Delegation,
            delegation_id: Some(job.id),
            created_by: SessionCreatedBy::Schedule,
        })
        .await
        .map_err(|err| WorkError::JobLane(err.to_string()))?;
    let mut harness = HarnessSettings::default();
    harness.loop_cfg.max_steps_per_turn = step_budget.max(1);
    let router = Arc::new(JobLayerRouter::new(
        Arc::clone(&host),
        registry,
        job.soul_id,
        job.id,
        &job.workspace_dir,
    ));
    let lane = LaneHandle::spawn(LaneOptions {
        store: Arc::clone(&sessions),
        session,
        soul: job.soul_id,
        model,
        harness,
        mind: MindSettings::default(),
        recovery: Vec::new(),
        speech: None,
        finalizer: None,
        prefetch: None,
        extra_context: Vec::new(),
        hooks: None,
        router: Some(router as Arc<dyn SurfaceRouter>),
    });
    let briefing = job_briefing(&job);
    if let Err(err) = lane.prompt_job(briefing, job.id).await {
        drop(host.fail(job.id, &err.to_string()));
        return Err(WorkError::JobLane(err.to_string()));
    }
    if let Err(err) = lane.wait_until_idle(wall).await {
        drop(lane.abort().await);
        drop(host.fail(job.id, "wall_timeout"));
        return Err(WorkError::JobLane(err.to_string()));
    }
    finish_if_still_open(&host, &sessions, session, &job)?;
    Ok(())
}

fn job_briefing(job: &Job) -> String {
    let mut text = format!(
        "You are the back-harness job lane.\nGoal: {}\nWorkspace: {}\n\
         Use tools as needed. Report with delegation.send kind=progress. \
         When finished, call delegation.send with kind=complete and a short summary.",
        job.goal, job.workspace_dir
    );
    if let Some(brief) = &job.brief
        && !brief.is_empty()
    {
        text.push_str("\nExcerpt:\n");
        text.push_str(brief);
    }
    text
}

fn finish_if_still_open(
    host: &DelegationHost,
    sessions: &SessionStore,
    session: ene_session::SessionId,
    job: &Job,
) -> Result<(), WorkError> {
    if let Some(current) = host.store().get_job(job.id)?
        && matches!(
            current.status,
            JobStatus::Completed
                | JobStatus::Cancelled
                | JobStatus::Failed
                | JobStatus::Interrupted
        )
    {
        return Ok(());
    }
    let summary = last_assistant_text(sessions, session).unwrap_or_else(|| job.goal.clone());
    match host.complete(job.id, &summary) {
        Ok(_) | Err(WorkError::AlreadyCompleted | WorkError::Cancelled) => Ok(()),
        Err(err) => Err(err),
    }
}

fn last_assistant_text(sessions: &SessionStore, session: ene_session::SessionId) -> Option<String> {
    let events = sessions.load_events(session, 0).ok()?;
    let history = derive_messages(&events, ProjectOptions::model_visible(32));
    history
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.text())
        .filter(|text| !text.is_empty())
}
