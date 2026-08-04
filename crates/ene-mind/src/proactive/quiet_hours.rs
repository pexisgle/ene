//! Deterministic quiet-hours evaluation for proactive speech.

use std::str::FromStr;

use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;

use crate::config::QuietHoursConfig;

/// Result of evaluating the quiet-hours window for one instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuietHoursEval {
    /// True when the configured window is active at the evaluated instant.
    pub active: bool,
    /// Local weekday (`monday`..`sunday`), stable English contract.
    pub weekday: String,
    /// Local wall date (`YYYY-MM-DD`) in the configured timezone.
    pub local_date: String,
    /// Local wall time as `HH:MM` in the configured timezone.
    pub local_time: String,
    /// Resolved timezone name (`Asia/Tokyo`, ...) or `local` for the system
    /// timezone.
    pub timezone: String,
    /// False when the configured timezone name is not a valid IANA zone.
    pub timezone_valid: bool,
}

impl QuietHoursEval {
    /// Inactive evaluation for an empty/disabled configuration.
    #[must_use]
    pub fn inactive() -> Self {
        Self::default()
    }
}

/// Evaluate the quiet-hours window for `now`.
///
/// The UTC instant is converted to local wall time in the configured IANA
/// timezone (or the system timezone when the config leaves it empty). The
/// conversion is unambiguous — one instant maps to exactly one wall time — so
/// a DST fall-back repeated hour counts as active for both occurrences and a
/// spring-forward skipped hour is never active.
///
/// Invalid timezones and invalid clock values fail safe to *inactive* (no
/// suppression) with a warning.
#[must_use]
pub fn evaluate_quiet_hours(config: &QuietHoursConfig, now: DateTime<Utc>) -> QuietHoursEval {
    if !config.enabled {
        return QuietHoursEval::inactive();
    }
    let Some((local, timezone)) = resolve_local_wall_time(config, now) else {
        return QuietHoursEval {
            timezone: config.timezone.clone(),
            timezone_valid: false,
            ..QuietHoursEval::inactive()
        };
    };
    let Some(minutes) = config.start.minutes_since_midnight() else {
        tracing::warn!(
            component = "Proactive",
            timezone = %timezone,
            "Quiet hours start time out of range; window treated as inactive"
        );
        return quiet_eval(&timezone, local, false);
    };
    let Some(end_minutes) = config.end.minutes_since_midnight() else {
        tracing::warn!(
            component = "Proactive",
            timezone = %timezone,
            "Quiet hours end time out of range; window treated as inactive"
        );
        return quiet_eval(&timezone, local, false);
    };
    let weekday = local.weekday();
    let local_minutes = local.hour() * 60 + local.minute();
    let in_window = match minutes.cmp(&end_minutes) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => {
            (minutes..end_minutes).contains(&local_minutes) && config.days.contains(weekday)
        }
        // Overnight: at/after start belongs to today's window (start day
        // enabled); before end belongs to the window that started yesterday
        // (the previous day's weekday must be enabled).
        std::cmp::Ordering::Greater => {
            (local_minutes >= minutes && config.days.contains(weekday))
                || (local_minutes < end_minutes && config.days.contains(weekday.pred()))
        }
    };
    quiet_eval(&timezone, local, in_window)
}

fn quiet_eval(timezone: &str, local: NaiveDateTime, active: bool) -> QuietHoursEval {
    QuietHoursEval {
        active,
        weekday: weekday_name(local.weekday()).to_string(),
        local_date: format!(
            "{:04}-{:02}-{:02}",
            local.year(),
            local.month(),
            local.day()
        ),
        local_time: format!("{:02}:{:02}", local.hour(), local.minute()),
        timezone: timezone.to_string(),
        timezone_valid: true,
    }
}

fn resolve_local_wall_time(
    config: &QuietHoursConfig,
    now: DateTime<Utc>,
) -> Option<(NaiveDateTime, String)> {
    if config.timezone.trim().is_empty() {
        // Resolve the system timezone once and convert the caller's instant
        // in it, so the offset honours the injected clock instead of the
        // wall-clock moment the evaluation happens to run at.
        let Some(tz_name) = system_timezone_name() else {
            tracing::warn!(
                component = "Proactive",
                "System timezone unavailable; quiet hours treated as inactive"
            );
            return None;
        };
        let Ok(tz) = Tz::from_str(&tz_name) else {
            tracing::warn!(
                component = "Proactive",
                timezone = %tz_name,
                "Resolved system timezone is not a known IANA zone; quiet hours treated as inactive"
            );
            return None;
        };
        return Some((
            tz.from_utc_datetime(&now.naive_utc()).naive_local(),
            "local".to_string(),
        ));
    }
    let timezone = config.timezone.trim();
    let Ok(tz) = Tz::from_str(timezone) else {
        tracing::warn!(
            component = "Proactive",
            timezone = %timezone,
            "Unknown IANA timezone; quiet hours treated as inactive"
        );
        return None;
    };
    Some((
        tz.from_utc_datetime(&now.naive_utc()).naive_local(),
        timezone.to_string(),
    ))
}

fn system_timezone_name() -> Option<String> {
    static SYSTEM_TIMEZONE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    SYSTEM_TIMEZONE
        .get_or_init(|| iana_time_zone::get_timezone().ok())
        .clone()
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{QuietHoursDaysConfig, QuietHoursSuppressConfig, QuietHoursTimeConfig};
    use chrono::TimeZone;

    fn config() -> QuietHoursConfig {
        QuietHoursConfig {
            enabled: true,
            timezone: "Asia/Tokyo".into(),
            days: QuietHoursDaysConfig {
                monday: true,
                ..QuietHoursDaysConfig::default()
            },
            start: QuietHoursTimeConfig {
                hour: 22,
                minute: 0,
            },
            end: QuietHoursTimeConfig { hour: 7, minute: 0 },
            suppress: QuietHoursSuppressConfig::default(),
            policy: crate::config::QuietHoursPolicy::default(),
        }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid utc instant")
    }

    #[test]
    fn disabled_config_is_never_active() {
        let mut cfg = config();
        cfg.enabled = false;
        let eval = evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 13, 0));
        assert!(!eval.active);
        assert!(eval.weekday.is_empty());
    }

    #[test]
    fn overnight_window_wraps_midnight() {
        // Mon 22:00–Tue 07:00 JST. Tokyo is UTC+9, no DST.
        let monday_22 = evaluate_quiet_hours(&config(), utc(2026, 8, 3, 13, 0));
        assert!(monday_22.active, "Mon 22:00 JST must be active");
        assert_eq!(monday_22.weekday, "monday");
        assert_eq!(monday_22.local_time, "22:00");

        let tuesday_02 = evaluate_quiet_hours(&config(), utc(2026, 8, 3, 17, 0));
        assert!(
            tuesday_02.active,
            "Tue 02:00 JST belongs to the Monday-night window"
        );
        assert_eq!(tuesday_02.weekday, "tuesday");

        // Monday 02:00 belongs to the Sunday-night window; sunday is not
        // enabled, so it must be inactive.
        let monday_02 = evaluate_quiet_hours(&config(), utc(2026, 8, 2, 17, 0));
        assert!(
            !monday_02.active,
            "Mon 02:00 JST belongs to the Sunday-night window"
        );

        // Enabling the previous day activates that morning portion.
        let mut sun_and_mon = config();
        sun_and_mon.days.sunday = true;
        let monday_02 = evaluate_quiet_hours(&sun_and_mon, utc(2026, 8, 2, 17, 0));
        assert!(monday_02.active);

        let tuesday_07 = evaluate_quiet_hours(&config(), utc(2026, 8, 3, 22, 0));
        assert!(!tuesday_07.active, "end is exclusive");
        assert_eq!(tuesday_07.local_time, "07:00");

        let tuesday_23 = evaluate_quiet_hours(&config(), utc(2026, 8, 4, 14, 0));
        assert!(
            !tuesday_23.active,
            "Tue 23:00 JST belongs to the Tuesday-night window, and tuesday is not enabled"
        );
    }

    #[test]
    fn same_day_window_uses_enabled_weekdays() {
        let mut cfg = config();
        cfg.days.tuesday = true;
        cfg.days.monday = false;
        cfg.start = QuietHoursTimeConfig { hour: 9, minute: 0 };
        cfg.end = QuietHoursTimeConfig {
            hour: 17,
            minute: 0,
        };

        let tue_noon = evaluate_quiet_hours(&cfg, utc(2026, 8, 4, 3, 0));
        assert!(tue_noon.active);
        assert_eq!(tue_noon.weekday, "tuesday");
        assert_eq!(tue_noon.local_time, "12:00");

        let mon_noon = evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 3, 0));
        assert!(!mon_noon.active, "monday is not enabled");

        let tue_1700 = evaluate_quiet_hours(&cfg, utc(2026, 8, 4, 8, 0));
        assert!(!tue_1700.active, "end is exclusive");
    }

    #[test]
    fn equal_start_and_end_is_an_empty_window() {
        let mut cfg = config();
        cfg.start = QuietHoursTimeConfig {
            hour: 12,
            minute: 0,
        };
        cfg.end = QuietHoursTimeConfig {
            hour: 12,
            minute: 0,
        };
        assert!(!evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 3, 0)).active);
    }

    #[test]
    fn invalid_timezone_fails_safe_to_inactive() {
        let mut cfg = config();
        cfg.timezone = "Not/AZone".into();
        let eval = evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 13, 0));
        assert!(!eval.active);
        assert!(!eval.timezone_valid);
    }

    #[test]
    fn empty_timezone_resolves_the_system_zone_at_the_injected_instant() {
        let Some(tz_name) = system_timezone_name() else {
            return; // exotic environments without a resolvable system zone
        };
        let tz: Tz = tz_name.parse().expect("resolved zone must parse");
        let mut cfg = config();
        cfg.timezone.clear();
        let now = utc(2026, 8, 3, 13, 0);
        let eval = evaluate_quiet_hours(&cfg, now);
        let expected = tz.from_utc_datetime(&now.naive_utc()).naive_local();
        assert!(eval.timezone_valid);
        assert_eq!(eval.local_date, expected.format("%Y-%m-%d").to_string());
        assert_eq!(eval.local_time, expected.format("%H:%M").to_string());
        assert_eq!(eval.weekday, weekday_name(expected.weekday()));
    }

    #[test]
    fn invalid_clock_values_fail_safe_to_inactive() {
        let mut cfg = config();
        cfg.start = QuietHoursTimeConfig {
            hour: 24,
            minute: 0,
        };
        let eval = evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 13, 0));
        assert!(!eval.active);
    }

    #[test]
    fn dst_spring_forward_skipped_hour_is_never_active() {
        // America/New_York spring-forward 2026-03-08: 02:00 EST jumps to
        // 03:00 EDT. A 01:00–04:00 window must be active before and after the
        // jump but the skipped 02:xx wall time never exists.
        let mut cfg = config();
        cfg.timezone = "America/New_York".into();
        cfg.days.sunday = true;
        cfg.days.monday = false;
        cfg.start = QuietHoursTimeConfig { hour: 1, minute: 0 };
        cfg.end = QuietHoursTimeConfig { hour: 4, minute: 0 };

        let before = evaluate_quiet_hours(&cfg, utc(2026, 3, 8, 6, 30));
        assert!(before.active);
        assert_eq!(before.local_time, "01:30");

        // 07:00 UTC = 03:00 EDT; 07:30 UTC = 03:30 EDT.
        let after = evaluate_quiet_hours(&cfg, utc(2026, 3, 8, 7, 30));
        assert!(after.active);
        assert_eq!(after.local_time, "03:30");

        // 06:30 UTC would be 01:30 EST but 02:30 EST does not exist; the
        // instant 07:59:59 UTC maps to 03:59:59 EDT (still inside).
        let edge = evaluate_quiet_hours(&cfg, utc(2026, 3, 8, 7, 59));
        assert!(edge.active);

        let outside = evaluate_quiet_hours(&cfg, utc(2026, 3, 8, 8, 0));
        assert!(!outside.active);
        assert_eq!(outside.local_time, "04:00");
    }

    #[test]
    fn dst_fall_back_repeated_hour_counts_for_both_occurrences() {
        // America/New_York fall-back 2026-11-01: 02:00 EDT repeats as 01:00
        // EST. A 01:00–02:00 window is active for both occurrences.
        let mut cfg = config();
        cfg.timezone = "America/New_York".into();
        cfg.days.sunday = true;
        cfg.days.monday = false;
        cfg.start = QuietHoursTimeConfig { hour: 1, minute: 0 };
        cfg.end = QuietHoursTimeConfig { hour: 2, minute: 0 };

        let first = evaluate_quiet_hours(&cfg, utc(2026, 11, 1, 5, 30));
        assert!(first.active, "01:30 EDT (first occurrence)");
        assert_eq!(first.local_time, "01:30");

        let second = evaluate_quiet_hours(&cfg, utc(2026, 11, 1, 6, 30));
        assert!(second.active, "01:30 EST (second occurrence)");
        assert_eq!(second.local_time, "01:30");

        let before_window = evaluate_quiet_hours(&cfg, utc(2026, 11, 1, 4, 59));
        assert!(!before_window.active, "00:59 EDT is before the window");
        let after = evaluate_quiet_hours(&cfg, utc(2026, 11, 1, 6, 59));
        assert!(after.active, "01:59 EST");
        let outside = evaluate_quiet_hours(&cfg, utc(2026, 11, 1, 7, 1));
        assert!(!outside.active, "02:01 EST");
    }

    #[test]
    fn weekday_is_checked_against_the_local_calendar() {
        let mut cfg = config();
        cfg.timezone = "Pacific/Auckland".into();
        cfg.days.sunday = true;
        cfg.days.monday = false;
        cfg.start = QuietHoursTimeConfig { hour: 0, minute: 0 };
        cfg.end = QuietHoursTimeConfig {
            hour: 23,
            minute: 59,
        };

        // 2026-08-03 12:00 UTC is 2026-08-04 00:00 NZST (Tuesday) — the local
        // calendar, not UTC, decides the weekday.
        let eval = evaluate_quiet_hours(&cfg, utc(2026, 8, 3, 12, 0));
        assert!(!eval.active);
        assert_eq!(eval.weekday, "tuesday");
    }
}
