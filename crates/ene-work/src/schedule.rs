use crate::error::WorkError;
use crate::store::{WorkStore, next_fire};
use crate::types::{CompanionReport, Schedule, ScheduleAction};
use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;

/// Quiet-hours window used only by schedules (not proactive).
#[derive(Debug, Clone)]
pub struct QuietWindow {
    pub enabled: bool,
    pub start_hour: u8,
    pub end_hour: u8,
    pub timezone: String,
}

impl Default for QuietWindow {
    fn default() -> Self {
        Self {
            enabled: false,
            start_hour: 22,
            end_hour: 7,
            timezone: String::new(),
        }
    }
}

pub fn fire_due(
    store: &WorkStore,
    now: DateTime<Utc>,
    quiet: &QuietWindow,
) -> Result<Vec<FiredSchedule>, WorkError> {
    let due = store.due_schedules(now)?;
    let mut out = Vec::new();
    for sched in due {
        if in_quiet(&sched, now, quiet) && !sched.important {
            let deferred = defer_past_quiet(now, quiet);
            store.defer_next_fire(&sched.id, deferred)?;
            continue;
        }
        store.mark_fired(&sched.id, now)?;
        out.push(FiredSchedule {
            schedule: sched,
            missed: false,
        });
    }
    Ok(out)
}

/// Startup: `remind` that missed its window fires once; `job`/`turn` do not (D-5).
///
/// This compatibility entry point keeps the historical no-quiet-hours behavior.
pub fn catch_up_missed(
    store: &WorkStore,
    now: DateTime<Utc>,
) -> Result<Vec<FiredSchedule>, WorkError> {
    catch_up_missed_with_quiet(store, now, &QuietWindow::default())
}

/// Startup catch-up with the same quiet-hours semantics as [`fire_due`].
/// Non-important reminders are deferred past quiet hours instead of being
/// marked fired and silently lost.
pub fn catch_up_missed_with_quiet(
    store: &WorkStore,
    now: DateTime<Utc>,
    quiet: &QuietWindow,
) -> Result<Vec<FiredSchedule>, WorkError> {
    let due = store.due_schedules(now)?;
    let mut out = Vec::new();
    for sched in due {
        match sched.action {
            ScheduleAction::Remind => {
                if in_quiet(&sched, now, quiet) && !sched.important {
                    let deferred = defer_past_quiet(now, quiet);
                    store.defer_next_fire(&sched.id, deferred)?;
                    continue;
                }
                store.mark_fired(&sched.id, now)?;
                out.push(FiredSchedule {
                    schedule: sched,
                    missed: true,
                });
            }
            ScheduleAction::Job | ScheduleAction::Turn => {
                let next = next_fire(&sched.spec, &sched.timezone, now)?;
                let next_at =
                    DateTime::parse_from_rfc3339(&next).map_or(now, |dt| dt.with_timezone(&Utc));
                store.defer_next_fire(&sched.id, next_at)?;
            }
        }
    }
    Ok(out)
}

pub fn reminder_report(sched: &Schedule) -> CompanionReport {
    let body = sched
        .action_ref
        .clone()
        .unwrap_or_else(|| sched.name.clone());
    CompanionReport {
        soul_id: sched.soul_id,
        speech: format!("it's time: {body}"),
        inner_intent: Some("remind".into()),
        starts_conversation: true,
    }
}

#[derive(Debug, Clone)]
pub struct FiredSchedule {
    pub schedule: Schedule,
    pub missed: bool,
}

fn in_quiet(sched: &Schedule, now: DateTime<Utc>, quiet: &QuietWindow) -> bool {
    if !quiet.enabled {
        return false;
    }
    let tz: Tz = if quiet.timezone.is_empty() {
        sched.timezone.parse().unwrap_or(chrono_tz::UTC)
    } else {
        quiet.timezone.parse().unwrap_or(chrono_tz::UTC)
    };
    let hour = now.with_timezone(&tz).hour() as u8;
    if quiet.start_hour == quiet.end_hour {
        return false;
    }
    if quiet.start_hour < quiet.end_hour {
        hour >= quiet.start_hour && hour < quiet.end_hour
    } else {
        hour >= quiet.start_hour || hour < quiet.end_hour
    }
}

fn defer_past_quiet(now: DateTime<Utc>, quiet: &QuietWindow) -> DateTime<Utc> {
    let tz: Tz = quiet.timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = now.with_timezone(&tz);
    let end = quiet.end_hour.min(23);
    let mut next = local
        .date_naive()
        .and_hms_opt(u32::from(end), 0, 0)
        .unwrap_or(local.naive_local());
    if next <= local.naive_local() {
        next += chrono::Duration::days(1);
    }
    next.and_local_timezone(tz)
        .single()
        .map_or(now + chrono::Duration::hours(1), |dt| {
            dt.with_timezone(&Utc)
        })
}
