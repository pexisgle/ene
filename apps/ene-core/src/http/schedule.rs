use std::time::Duration;

use chrono::Utc;
use ene_companion::MindSettings;
use ene_work::{
    DelegationMode, FiredSchedule, QuietWindow, ScheduleAction, StartDelegation,
    catch_up_missed_with_quiet, fire_due, reminder_report,
};

use super::AppState;

const POLL: Duration = Duration::from_secs(1);

pub async fn run_loop(state: AppState) {
    tick(&state, true).await;
    loop {
        tokio::time::sleep(POLL).await;
        tick(&state, false).await;
    }
}

async fn tick(state: &AppState, catch_up: bool) {
    let work = state.core.work();
    let now = Utc::now();
    let quiet = quiet_from_mind(&state.core.mind());
    let fired = if catch_up {
        match catch_up_missed_with_quiet(&work, now, &quiet) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "schedule catch-up failed");
                return;
            }
        }
    } else {
        match fire_due(&work, now, &quiet) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, "schedule fire failed");
                return;
            }
        }
    };
    for row in fired {
        dispatch(state, row).await;
    }
}

fn quiet_from_mind(mind: &MindSettings) -> QuietWindow {
    let hours = &mind.proactive.quiet_hours;
    QuietWindow {
        enabled: hours.enabled,
        start_hour: hours.start.hour,
        end_hour: hours.end.hour,
        timezone: hours.timezone.clone(),
    }
}

async fn dispatch(state: &AppState, fired: FiredSchedule) {
    match fired.schedule.action {
        ScheduleAction::Remind => {
            let report = reminder_report(&fired.schedule);
            drop(state.core.host().deliver_companion_report(report));
        }
        ScheduleAction::Job => start_scheduled_job(state, &fired),
        ScheduleAction::Turn => start_scheduled_turn(state, &fired).await,
    }
}

fn start_scheduled_job(state: &AppState, fired: &FiredSchedule) {
    let goal = fired
        .schedule
        .action_ref
        .clone()
        .unwrap_or_else(|| fired.schedule.name.clone());
    if let Err(err) = state.core.host().start(StartDelegation {
        soul_id: fired.schedule.soul_id,
        goal,
        mode: DelegationMode::Public,
        title: Some(fired.schedule.name.clone()),
        brief: None,
        plan: None,
        created_from_turn: None,
        depth: 0,
        parent_id: None,
    }) {
        tracing::warn!(
            error = %err,
            name = %fired.schedule.name,
            "scheduled job failed to start"
        );
    }
}

async fn start_scheduled_turn(state: &AppState, fired: &FiredSchedule) {
    let hint = fired
        .schedule
        .action_ref
        .clone()
        .unwrap_or_else(|| fired.schedule.name.clone());
    let Ok(sessions) = state
        .core
        .store()
        .list_sessions(Some(fired.schedule.soul_id))
    else {
        return;
    };
    let Some(session) = sessions.iter().find(|meta| meta.ended_at.is_none()) else {
        tracing::debug!(
            name = %fired.schedule.name,
            "scheduled turn skipped: no open session"
        );
        return;
    };
    match state.lanes.get_or_open(&state.core, session.id) {
        Ok(lane) => {
            if let Err(err) = lane.scheduled(hint).await {
                tracing::warn!(error = %err, "scheduled turn failed");
            }
        }
        Err(err) => tracing::warn!(error = %err, "scheduled turn has no lane"),
    }
}
