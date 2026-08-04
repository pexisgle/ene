//! Pure wall-clock computation for persistent schedules.
//!
//! Lives here (rather than `ene-store` or `ene-runtime`) so the store can
//! advance a schedule's `next_run_at` inside the same transaction that
//! claims a fire, and so the DST/timezone behavior is unit-testable without
//! a database.

use super::{NewSchedule, Schedule, ScheduleError, ScheduleKind};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use cron::Schedule as CronSchedule;
use std::str::FromStr;

fn parse_timezone(value: &str) -> Result<Tz, ScheduleError> {
    Tz::from_str(value).map_err(|e| ScheduleError::InvalidTimezone {
        value: value.to_string(),
        detail: e.clone(),
    })
}

fn parse_cron(value: &str) -> Result<CronSchedule, ScheduleError> {
    // cron 0.17 requires the seconds field; accept the common 5-field form
    // by defaulting seconds to 0.
    let normalized = if value.split_whitespace().count() == 5 {
        format!("0 {value}")
    } else {
        value.to_string()
    };
    CronSchedule::from_str(&normalized).map_err(|e| ScheduleError::InvalidCron {
        value: value.to_string(),
        detail: e.to_string(),
    })
}

/// Validates a new schedule and computes its first `next_run_at`.
pub fn first_run_at(new: &NewSchedule, now: DateTime<Utc>) -> Result<DateTime<Utc>, ScheduleError> {
    if new.name.trim().is_empty() {
        return Err(ScheduleError::EmptyName);
    }
    let tz = parse_timezone(&new.timezone)?;
    match new.kind {
        ScheduleKind::OneShot => {
            let start_at = new.start_at.ok_or(ScheduleError::MissingField {
                field: "start_at",
                kind: "one_shot",
            })?;
            if start_at <= now {
                return Err(ScheduleError::InvalidStartAt { value: start_at });
            }
            Ok(start_at)
        }
        ScheduleKind::Startup => Ok(now),
        ScheduleKind::Interval => {
            let anchor = new.start_at.ok_or(ScheduleError::MissingField {
                field: "start_at",
                kind: "interval",
            })?;
            let interval = new.interval_secs.ok_or(ScheduleError::MissingField {
                field: "interval_secs",
                kind: "interval",
            })?;
            if interval <= 0 {
                return Err(ScheduleError::InvalidInterval { value: interval });
            }
            Ok(interval_tick_at_or_after(anchor, interval, now))
        }
        ScheduleKind::Cron => {
            let expr = new
                .cron_expr
                .as_deref()
                .ok_or(ScheduleError::MissingField {
                    field: "cron_expr",
                    kind: "cron",
                })?;
            let sched = parse_cron(expr)?;
            next_cron_after(&sched, tz, now).ok_or(ScheduleError::NoNextOccurrence)
        }
    }
}

/// The strictly-next occurrence after `after` for an existing schedule.
///
/// `None` means the schedule has no further occurrences: one-shot and
/// startup schedules complete after their single fire.
pub fn next_occurrence_after(s: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match s.kind {
        ScheduleKind::OneShot | ScheduleKind::Startup => None,
        ScheduleKind::Interval => {
            let anchor = s.start_at?;
            let interval = s.interval_secs?;
            if interval <= 0 {
                return None;
            }
            Some(interval_tick_strictly_after(anchor, interval, after))
        }
        ScheduleKind::Cron => {
            let tz = parse_timezone(&s.timezone).ok()?;
            let sched = parse_cron(s.cron_expr.as_deref()?).ok()?;
            next_cron_after(&sched, tz, after)
        }
    }
}

/// The first fixed-rate tick at or after `after`, anchored on `anchor`.
fn interval_tick_at_or_after(
    anchor: DateTime<Utc>,
    interval_secs: i64,
    after: DateTime<Utc>,
) -> DateTime<Utc> {
    let elapsed = (after - anchor).num_seconds();
    let ticks = if elapsed <= 0 {
        0
    } else {
        elapsed / interval_secs + i64::from(elapsed % interval_secs != 0)
    };
    anchor + Duration::seconds(ticks * interval_secs)
}

/// The first fixed-rate tick strictly after `after`, anchored on `anchor`.
fn interval_tick_strictly_after(
    anchor: DateTime<Utc>,
    interval_secs: i64,
    after: DateTime<Utc>,
) -> DateTime<Utc> {
    let elapsed = (after - anchor).num_seconds();
    let ticks = elapsed.div_euclid(interval_secs) + 1;
    anchor + Duration::seconds(ticks.max(1) * interval_secs)
}

fn next_cron_after(sched: &CronSchedule, tz: Tz, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let local_after = after.with_timezone(&tz);
    sched
        .after_owned(local_after)
        .next()
        .map(|t| t.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ScheduleAction, ScheduleConfirmation};
    use chrono::TimeZone;

    fn new_schedule(
        kind: ScheduleKind,
        timezone: &str,
        cron_expr: Option<&str>,
        interval_secs: Option<i64>,
        start_at: Option<DateTime<Utc>>,
    ) -> NewSchedule {
        NewSchedule {
            name: "test".to_string(),
            kind,
            timezone: timezone.to_string(),
            cron_expr: cron_expr.map(str::to_string),
            interval_secs,
            start_at,
            action: ScheduleAction::Prompt {
                text: "hello".to_string(),
                allow_tools: false,
            },
            confirmation: ScheduleConfirmation::None,
            max_retries: 0,
            retry_delay_secs: 60,
        }
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid instant")
    }

    #[test]
    fn one_shot_first_run_is_start_at() {
        let now = at(2026, 8, 4, 12, 0);
        let start = at(2026, 8, 4, 15, 30);
        let new = new_schedule(ScheduleKind::OneShot, "UTC", None, None, Some(start));
        assert_eq!(first_run_at(&new, now).expect("valid"), start);
    }

    #[test]
    fn one_shot_start_at_in_past_is_rejected() {
        let now = at(2026, 8, 4, 12, 0);
        let start = at(2026, 8, 4, 11, 59);
        let new = new_schedule(ScheduleKind::OneShot, "UTC", None, None, Some(start));
        assert!(matches!(
            first_run_at(&new, now),
            Err(ScheduleError::InvalidStartAt { .. })
        ));
    }

    #[test]
    fn one_shot_has_no_next_occurrence() {
        let now = at(2026, 8, 4, 12, 0);
        let start = at(2026, 8, 4, 15, 30);
        let new = new_schedule(ScheduleKind::OneShot, "UTC", None, None, Some(start));
        let schedule = Schedule {
            id: 1,
            name: new.name.clone(),
            kind: new.kind,
            enabled: true,
            timezone: new.timezone.clone(),
            cron_expr: new.cron_expr.clone(),
            interval_secs: new.interval_secs,
            start_at: new.start_at,
            action: new.action.clone(),
            confirmation: new.confirmation,
            max_retries: new.max_retries,
            retry_delay_secs: new.retry_delay_secs,
            next_run_at: Some(start),
            pending_retry_of_run_id: None,
            last_run_at: None,
            last_status: None,
            run_count: 0,
            fail_count: 0,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(next_occurrence_after(&schedule, start), None);
        assert_eq!(next_occurrence_after(&schedule, now), None);
    }

    #[test]
    fn interval_ticks_from_anchor_at_fixed_rate() {
        let anchor = at(2026, 8, 4, 0, 0);
        let new = new_schedule(
            ScheduleKind::Interval,
            "UTC",
            None,
            Some(3600),
            Some(anchor),
        );
        // Creation at 02:30 with a midnight anchor: first tick is 03:00.
        assert_eq!(
            first_run_at(&new, at(2026, 8, 4, 2, 30)).expect("valid"),
            at(2026, 8, 4, 3, 0)
        );
        let schedule = Schedule {
            id: 1,
            name: new.name.clone(),
            kind: new.kind,
            enabled: true,
            timezone: new.timezone.clone(),
            cron_expr: new.cron_expr.clone(),
            interval_secs: new.interval_secs,
            start_at: new.start_at,
            action: new.action.clone(),
            confirmation: new.confirmation,
            max_retries: new.max_retries,
            retry_delay_secs: new.retry_delay_secs,
            next_run_at: Some(at(2026, 8, 4, 3, 0)),
            pending_retry_of_run_id: None,
            last_run_at: None,
            last_status: None,
            run_count: 0,
            fail_count: 0,
            created_at: at(2026, 8, 4, 2, 30),
            updated_at: at(2026, 8, 4, 2, 30),
        };
        // After a fire at 03:00 the next tick is 04:00; after a missed 03:00
        // the next tick from 03:45 is 04:00.
        assert_eq!(
            next_occurrence_after(&schedule, at(2026, 8, 4, 3, 0)),
            Some(at(2026, 8, 4, 4, 0))
        );
        assert_eq!(
            next_occurrence_after(&schedule, at(2026, 8, 4, 3, 45)),
            Some(at(2026, 8, 4, 4, 0))
        );
    }

    #[test]
    fn cron_daily_utc() {
        let new = new_schedule(ScheduleKind::Cron, "UTC", Some("0 9 * * *"), None, None);
        let now = at(2026, 8, 4, 8, 0);
        assert_eq!(
            first_run_at(&new, now).expect("valid"),
            at(2026, 8, 4, 9, 0)
        );
        let schedule = Schedule {
            id: 1,
            name: new.name.clone(),
            kind: new.kind,
            enabled: true,
            timezone: new.timezone.clone(),
            cron_expr: new.cron_expr.clone(),
            interval_secs: new.interval_secs,
            start_at: new.start_at,
            action: new.action.clone(),
            confirmation: new.confirmation,
            max_retries: new.max_retries,
            retry_delay_secs: new.retry_delay_secs,
            next_run_at: Some(at(2026, 8, 4, 9, 0)),
            pending_retry_of_run_id: None,
            last_run_at: None,
            last_status: None,
            run_count: 0,
            fail_count: 0,
            created_at: now,
            updated_at: now,
        };
        assert_eq!(
            next_occurrence_after(&schedule, at(2026, 8, 4, 9, 0)),
            Some(at(2026, 8, 4, 9, 0) + Duration::days(1))
        );
    }

    #[test]
    fn cron_seconds_field_supported() {
        let new = new_schedule(
            ScheduleKind::Cron,
            "UTC",
            Some("*/30 * * * * *"),
            None,
            None,
        );
        let now = at(2026, 8, 4, 12, 0);
        assert_eq!(
            first_run_at(&new, now).expect("valid"),
            at(2026, 8, 4, 12, 0) + Duration::seconds(30)
        );
    }

    #[test]
    fn cron_invalid_expression_rejected() {
        let new = new_schedule(ScheduleKind::Cron, "UTC", Some("not a cron"), None, None);
        assert!(matches!(
            first_run_at(&new, at(2026, 8, 4, 12, 0)),
            Err(ScheduleError::InvalidCron { .. })
        ));
    }

    #[test]
    fn invalid_timezone_rejected() {
        let new = new_schedule(
            ScheduleKind::Cron,
            "Mars/Olympus",
            Some("0 9 * * *"),
            None,
            None,
        );
        assert!(matches!(
            first_run_at(&new, at(2026, 8, 4, 12, 0)),
            Err(ScheduleError::InvalidTimezone { .. })
        ));
    }

    #[test]
    fn cron_asia_tokyo_is_utc_minus_nine() {
        let new = new_schedule(
            ScheduleKind::Cron,
            "Asia/Tokyo",
            Some("0 9 * * *"),
            None,
            None,
        );
        let now = at(2026, 8, 4, 0, 0);
        // 09:00 JST = 00:00 UTC, which is strictly after `now`, so the first
        // occurrence is the next day's 09:00 JST = 2026-08-05T00:00:00Z.
        assert_eq!(
            first_run_at(&new, now).expect("valid"),
            at(2026, 8, 5, 0, 0)
        );
    }

    #[test]
    fn cron_spring_forward_skips_nonexistent_local_time() {
        // America/New_York 2026-03-08: 02:00 EST jumps to 03:00 EDT.
        // A daily 02:30 job has no 02:30 on that day; the next valid local
        // 02:30 is the day after.
        let new = new_schedule(
            ScheduleKind::Cron,
            "America/New_York",
            Some("30 2 * * *"),
            None,
            None,
        );
        let before_transition = at(2026, 3, 8, 1, 0);
        let first = first_run_at(&new, before_transition).expect("valid");
        // 02:30 does not exist on 2026-03-08; the first valid occurrence is
        // 2026-03-09 02:30 EDT = 06:30 UTC.
        assert_eq!(first, at(2026, 3, 9, 6, 30));
    }

    #[test]
    fn cron_fall_back_keeps_local_wall_clock() {
        // America/New_York 2026-11-01: 02:00 EDT falls back to 01:00 EST.
        // A daily 01:30 job fires twice that day in local terms (one in each
        // offset); iteration must not skip the second (earlier-offset) fold.
        let new = new_schedule(
            ScheduleKind::Cron,
            "America/New_York",
            Some("30 1 * * *"),
            None,
            None,
        );
        let now = at(2026, 10, 31, 2, 0);
        let first = first_run_at(&new, now).expect("valid");
        // The next 01:30 local is 2026-10-31 01:30 EDT = 05:30 UTC.
        assert_eq!(first, at(2026, 10, 31, 5, 30));
        let schedule = Schedule {
            id: 1,
            name: new.name.clone(),
            kind: new.kind,
            enabled: true,
            timezone: new.timezone.clone(),
            cron_expr: new.cron_expr.clone(),
            interval_secs: new.interval_secs,
            start_at: new.start_at,
            action: new.action.clone(),
            confirmation: new.confirmation,
            max_retries: new.max_retries,
            retry_delay_secs: new.retry_delay_secs,
            next_run_at: Some(first),
            pending_retry_of_run_id: None,
            last_run_at: None,
            last_status: None,
            run_count: 0,
            fail_count: 0,
            created_at: now,
            updated_at: now,
        };
        // 2026-11-01 has two 01:30 local instants (EDT fold then EST fold).
        // The iterator must emit both: 05:30 UTC then 06:30 UTC.
        let fold_first = next_occurrence_after(&schedule, first).expect("fold first");
        assert_eq!(fold_first, at(2026, 11, 1, 5, 30));
        assert_eq!(
            next_occurrence_after(&schedule, fold_first),
            Some(at(2026, 11, 1, 6, 30))
        );
        // Cadence continues the next day in the post-transition offset.
        assert_eq!(
            next_occurrence_after(&schedule, at(2026, 11, 1, 6, 30)),
            Some(at(2026, 11, 2, 6, 30))
        );
    }

    #[test]
    fn startup_fires_at_creation_instant() {
        let new = new_schedule(ScheduleKind::Startup, "UTC", None, None, None);
        let now = at(2026, 8, 4, 12, 0);
        assert_eq!(first_run_at(&new, now).expect("valid"), now);
    }

    #[test]
    fn empty_name_rejected() {
        let mut new = new_schedule(ScheduleKind::Startup, "UTC", None, None, None);
        new.name = "   ".to_string();
        assert_eq!(
            first_run_at(&new, at(2026, 8, 4, 12, 0)),
            Err(ScheduleError::EmptyName)
        );
    }
}
