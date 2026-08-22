use std::time::Duration;

use super::AppState;
use super::routes::{emit_job_reports, persist_job_report};

#[cfg(test)]
const TICK: Duration = Duration::from_millis(50);
#[cfg(not(test))]
const TICK: Duration = Duration::from_secs(30);

/// Drive unanswered ask-user timeouts until serve shutdown aborts this task.
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
    emit_job_reports(state, &reports);
    for report in &reports {
        persist_job_report(state, report).await;
    }
}
