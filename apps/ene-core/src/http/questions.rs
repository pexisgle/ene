use std::time::Duration;

use super::AppState;
use super::routes::{emit_job_reports, emit_question_resolved, persist_job_report};

#[cfg(test)]
const TICK: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const TICK: Duration = Duration::from_secs(30);

/// Drive unanswered ask-user timeouts until serve shutdown aborts this task.
///
/// Deadlines are wall-clock timestamps on the mailbox question row, so a
/// core restart mid-question neither resets nor doubles the wait: the tick
/// compares `Utc::now` against the original `asked_at`, and the assumption
/// push closes the question exactly once.
pub async fn run_loop(state: AppState) {
    loop {
        tokio::time::sleep(TICK).await;
        tick(&state).await;
    }
}

pub(crate) async fn tick(state: &AppState) {
    let reports = match state
        .core
        .host()
        .resolve_question_timeouts(chrono::Utc::now(), None)
    {
        Ok(reports) => reports,
        Err(err) => {
            tracing::warn!(error = %err, "question timeout tick failed");
            return;
        }
    };
    if reports.is_empty() {
        return;
    }
    let mut resolved_jobs = Vec::new();
    for report in &reports {
        if let Some(job_id) = report.job_id
            && !resolved_jobs.contains(&job_id)
        {
            resolved_jobs.push(job_id);
        }
    }
    emit_job_reports(state, &reports);
    for job_id in resolved_jobs {
        if state
            .core
            .host()
            .open_questions(job_id)
            .is_ok_and(|questions| questions.is_empty())
        {
            emit_question_resolved(state, job_id);
        }
    }
    for report in &reports {
        persist_job_report(state, report).await;
    }
}
