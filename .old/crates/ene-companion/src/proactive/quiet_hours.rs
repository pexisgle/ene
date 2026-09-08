use crate::config::QuietHoursSettings;
use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use std::str::FromStr;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuietHoursEval {
    pub active: bool,
    pub weekday: String,
    pub local_time: String,
    pub timezone: String,
}

impl QuietHoursEval {
    #[must_use]
    pub fn inactive() -> Self {
        Self::default()
    }
}

#[must_use]
pub fn evaluate_quiet_hours(config: &QuietHoursSettings, now: DateTime<Utc>) -> QuietHoursEval {
    if !config.enabled {
        return QuietHoursEval::inactive();
    }
    let Some((local, timezone)) = resolve_local(config, now) else {
        return QuietHoursEval {
            timezone: config.timezone.clone(),
            ..QuietHoursEval::inactive()
        };
    };
    let Some(start) = config.start.minutes_since_midnight() else {
        return quiet_eval(&timezone, local, false);
    };
    let Some(end) = config.end.minutes_since_midnight() else {
        return quiet_eval(&timezone, local, false);
    };
    let weekday = local.weekday();
    let local_minutes = local.hour() * 60 + local.minute();
    let in_window = match start.cmp(&end) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => {
            (start..end).contains(&local_minutes) && config.days.contains(weekday)
        }
        std::cmp::Ordering::Greater => {
            (local_minutes >= start && config.days.contains(weekday))
                || (local_minutes < end && config.days.contains(weekday.pred()))
        }
    };
    quiet_eval(&timezone, local, in_window)
}

fn quiet_eval(timezone: &str, local: NaiveDateTime, active: bool) -> QuietHoursEval {
    QuietHoursEval {
        active,
        weekday: weekday_name(local.weekday()).to_owned(),
        local_time: format!("{:02}:{:02}", local.hour(), local.minute()),
        timezone: timezone.to_owned(),
    }
}

fn resolve_local(
    config: &QuietHoursSettings,
    now: DateTime<Utc>,
) -> Option<(NaiveDateTime, String)> {
    if config.timezone.trim().is_empty() {
        return Some(system_local(now));
    }
    let timezone = config.timezone.trim();
    let Ok(tz) = Tz::from_str(timezone) else {
        return None;
    };
    Some((
        tz.from_utc_datetime(&now.naive_utc()).naive_local(),
        timezone.to_owned(),
    ))
}

fn system_local(now: DateTime<Utc>) -> (NaiveDateTime, String) {
    if let Ok(name) = iana_time_zone::get_timezone()
        && let Ok(tz) = Tz::from_str(&name)
    {
        return (
            tz.from_utc_datetime(&now.naive_utc()).naive_local(),
            "local".to_owned(),
        );
    }
    (
        chrono::Local
            .from_utc_datetime(&now.naive_utc())
            .naive_local(),
        "local".to_owned(),
    )
}

const fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}
