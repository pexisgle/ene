//! Natural-language due-date parsing for commitment candidates.
//!
//! The LLM extractor records `commitment_due` as a free-form hint ("tomorrow
//! at 3pm", "来週の金曜"). This module turns the common English and Japanese
//! relative expressions into an absolute UTC deadline so the ledger can order
//! rows by `due_at`, auto-stale them via [`crate::commitments`]'s
//! `mark_stale_overdue`, and render relative deadlines in prompts.
//!
//! Unparseable hints (e.g. "next time", "次回") yield `None` and the raw label
//! is preserved for display.

use chrono::{DateTime, Datelike, Duration, Months, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use regex::Regex;
use std::str::FromStr;
use std::sync::LazyLock;

/// End-of-day wall time used for date-only expressions ("tomorrow").
fn end_of_day() -> NaiveTime {
    NaiveTime::from_hms_opt(23, 59, 0).unwrap_or(NaiveTime::MIN)
}

/// 12-hour clock, e.g. "3pm", "3 pm", "3:30pm".
static MERIDIEM_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2})(?::(\d{2}))?\s*(a\.?m\.?|p\.?m\.?)\b")
        .unwrap_or_else(|_| unreachable!("static meridiem regex is valid"))
});

/// `HH:MM` (24-hour clock), e.g. "15:00", "9:30".
static CLOCK_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2}):(\d{2})\b").unwrap_or_else(|_| unreachable!("static clock regex"))
});

/// Japanese clock, e.g. "3時", "15時半", "午後3時".
static JAPANESE_TIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(午前|午後)?(\d{1,2})時(半)?")
        .unwrap_or_else(|_| unreachable!("static japanese time regex is valid"))
});

/// Duration expression, e.g. "in 3 days", "5 weeks from now", "3日後".
///
/// The Japanese units are matched without a trailing `\b` because they are
/// non-ASCII and the regex crate's word boundary is ASCII-only.
static DURATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(\d+)\s*(days?|weeks?|hours?|minutes?|months?)\b|(\d+)\s*(日後|週間後|時間後|分後|か月後|ヶ月後)",
    )
    .unwrap_or_else(|_| unreachable!("static duration regex is valid"))
});

/// Parse `raw` against the system timezone.
///
/// See [`parse_due_at_in`] for the supported expressions.
pub fn parse_due_at(now: DateTime<Utc>, raw: &str) -> Option<DateTime<Utc>> {
    let tz = system_timezone();
    parse_due_at_in(now, tz, raw)
}

/// Parse `raw` into an absolute UTC deadline relative to `now` and `tz`.
///
/// Recognized expressions (English and Japanese):
/// - date terms: today, tonight, tomorrow, day after tomorrow, next
///   week/month/year; 今日, 今晩, 明日, 明後日, 来週, 来月, 来年
/// - weekdays: monday..sunday with optional this/next; X曜日, 今週のX曜日,
///   来週のX曜日, 次のX曜日
/// - durations: in N days/weeks/hours/minutes, N days from now, N日後,
///   N週間後, N時間後, N分後, Nか月後
/// - times: HH:MM, N am/pm, N時(半), 午前/午後N時
///
/// Date-only terms resolve to 23:59 local; duration terms keep the current
/// time of day. Leading "by"/"までに" and trailing "まで" are ignored.
pub fn parse_due_at_in(now: DateTime<Utc>, tz: Tz, raw: &str) -> Option<DateTime<Utc>> {
    let text = normalize(raw);
    if text.is_empty() {
        return None;
    }

    let time = extract_time(&text);
    let local_now = tz.from_utc_datetime(&now.naive_utc()).naive_local();

    let (local_date, time_of_day) = resolve_date_term(&text, local_now, time)?;
    let naive = local_date.and_time(time_of_day.unwrap_or_else(end_of_day));
    let local = tz.from_local_datetime(&naive).earliest()?;
    Some(local.with_timezone(&Utc))
}

/// Lowercases, trims, strips leading "by"/"までに" and trailing punctuation /
/// "まで", then collapses whitespace.
fn normalize(raw: &str) -> String {
    let mut text = raw.trim().to_lowercase();
    if let Some(rest) = text.strip_prefix("by ") {
        text = rest.to_string();
    } else if let Some(rest) = text.strip_prefix("までに ") {
        text = rest.to_string();
    }
    if let Some(rest) = text.strip_suffix("までに") {
        text = rest.to_string();
    } else if let Some(rest) = text.strip_suffix("まで") {
        text = rest.to_string();
    }
    let text = text.trim_end_matches(['.', '!', '？', '！', '。', ',', '、']);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extracts an explicit wall-clock time from `text`, if any.
fn extract_time(text: &str) -> Option<NaiveTime> {
    if let Some(caps) = MERIDIEM_TIME.captures(text) {
        let mut hour = caps.get(1)?.as_str().parse::<u32>().ok()?;
        let minute = caps
            .get(2)
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        let is_pm = caps.get(3)?.as_str().starts_with('p');
        if is_pm && hour < 12 {
            hour += 12;
        } else if !is_pm && hour == 12 {
            hour = 0;
        }
        return NaiveTime::from_hms_opt(hour, minute, 0);
    }
    if let Some(caps) = CLOCK_TIME.captures(text) {
        let hour = caps.get(1)?.as_str().parse::<u32>().ok()?;
        let minute = caps.get(2)?.as_str().parse::<u32>().ok()?;
        return NaiveTime::from_hms_opt(hour, minute, 0);
    }
    if let Some(caps) = JAPANESE_TIME.captures(text) {
        let mut hour = caps.get(2)?.as_str().parse::<u32>().ok()?;
        let half = caps.get(3).is_some();
        if caps.get(1).is_some_and(|m| m.as_str() == "午後") && hour < 12 {
            hour += 12;
        }
        let minute = if half { 30 } else { 0 };
        return NaiveTime::from_hms_opt(hour, minute, 0);
    }
    None
}

/// Resolves the calendar date the expression refers to, plus an optional
/// explicit time. `None` when no term is recognized.
fn resolve_date_term(
    text: &str,
    local_now: chrono::NaiveDateTime,
    time: Option<NaiveTime>,
) -> Option<(chrono::NaiveDate, Option<NaiveTime>)> {
    let today = local_now.date();
    let day = |shift: i64| Some((today + Duration::days(shift), time));

    // Weekday terms first: "来週の金曜日" contains "来週" and must not be
    // swallowed by the generic "next week" rule.
    if let Some(date) = weekday_term(text, today) {
        return Some((date, time));
    }
    if contains_any(text, &["today", "tonight", "今日", "今晩"]) {
        return day(0);
    }
    if contains_any(text, &["day after tomorrow", "明後日"]) {
        return day(2);
    }
    if contains_any(text, &["tomorrow", "明日"]) {
        return day(1);
    }
    if contains_any(text, &["next week", "来週"]) {
        return day(7);
    }
    if contains_any(text, &["next month", "来月"]) {
        return Some((add_months(today, 1), time));
    }
    if contains_any(text, &["next year", "来年"]) {
        return Some((add_months(today, 12), time));
    }

    if let Some((shift, kind)) = duration_term(text) {
        let delta = match kind {
            DurationKind::Day => Duration::days(shift),
            DurationKind::Week => Duration::weeks(shift),
            DurationKind::Hour => Duration::hours(shift),
            DurationKind::Minute => Duration::minutes(shift),
            DurationKind::Month => {
                let shifted = add_months(today, shift as u32);
                return Some((shifted, Some(time.unwrap_or_else(|| local_now.time()))));
            }
        };
        // Without an explicit wall time the whole `now + delta` instant is
        // kept; with one, the shifted date carries that wall time instead.
        return if let Some(t) = time {
            Some((today + delta, Some(t)))
        } else {
            let shifted = local_now + delta;
            Some((shifted.date(), Some(shifted.time())))
        };
    }

    None
}

/// Matches "in N days" / "N days from now" / "N日後" style expressions.
enum DurationKind {
    Day,
    Week,
    Hour,
    Minute,
    Month,
}

fn duration_term(text: &str) -> Option<(i64, DurationKind)> {
    let caps = DURATION.captures(text)?;
    let (shift, unit) = if let (Some(shift), Some(unit)) = (caps.get(1), caps.get(2)) {
        (shift.as_str(), unit.as_str())
    } else if let (Some(shift), Some(unit)) = (caps.get(3), caps.get(4)) {
        (shift.as_str(), unit.as_str())
    } else {
        return None;
    };
    let shift = shift.parse::<i64>().ok()?;
    let unit = unit.strip_suffix('s').unwrap_or(unit);
    let kind = match unit {
        "day" | "日後" => DurationKind::Day,
        "week" | "週間後" => DurationKind::Week,
        "hour" | "時間後" => DurationKind::Hour,
        "minute" | "分後" => DurationKind::Minute,
        "month" | "か月後" | "ヶ月後" => DurationKind::Month,
        _ => return None,
    };
    Some((shift, kind))
}

/// Matches weekday expressions ("friday", "next monday", "金曜日",
/// "来週の月曜日", "次の火曜"). Bare / "this" weekdays resolve to the next
/// occurrence at or after today; "next"/"来週の"/"次の" resolve to the same
/// weekday in the following ISO week.
fn weekday_term(text: &str, today: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
    const DAYS: &[(&str, Weekday)] = &[
        ("sunday", Weekday::Sun),
        ("monday", Weekday::Mon),
        ("tuesday", Weekday::Tue),
        ("wednesday", Weekday::Wed),
        ("thursday", Weekday::Thu),
        ("friday", Weekday::Fri),
        ("saturday", Weekday::Sat),
    ];
    for &(name, weekday) in DAYS {
        let next = text.contains("next ");
        let mut strict = next && text.contains(name);
        let mut matched = strict || text.contains(name);
        // Japanese: "X曜日", optionally prefixed with 今週の / 来週の / 次の.
        let ja_name = japanese_weekday(weekday);
        if text.contains(ja_name) {
            matched = true;
            if text.contains("来週の") || text.contains("次の") {
                strict = true;
            }
        }
        if matched {
            return Some(next_weekday(today, weekday, strict));
        }
    }
    None
}

fn japanese_weekday(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "月曜日",
        Weekday::Tue => "火曜日",
        Weekday::Wed => "水曜日",
        Weekday::Thu => "木曜日",
        Weekday::Fri => "金曜日",
        Weekday::Sat => "土曜日",
        Weekday::Sun => "日曜日",
    }
}

/// Next occurrence of `weekday`: at or after `today` when `strict` is false,
/// in the following ISO week when true.
fn next_weekday(today: chrono::NaiveDate, weekday: Weekday, strict: bool) -> chrono::NaiveDate {
    let offset = (i64::from(weekday.num_days_from_monday())
        - i64::from(today.weekday().num_days_from_monday()))
    .rem_euclid(7);
    let delta = if strict { 7 + offset } else { offset };
    today + Duration::days(delta)
}

fn add_months(today: chrono::NaiveDate, months: u32) -> chrono::NaiveDate {
    today
        .checked_add_months(Months::new(months))
        .unwrap_or(today)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

/// System IANA timezone name, cached per process.
fn system_timezone() -> Tz {
    static SYSTEM_TIMEZONE: std::sync::OnceLock<Tz> = std::sync::OnceLock::new();
    *SYSTEM_TIMEZONE.get_or_init(|| {
        iana_time_zone::get_timezone()
            .ok()
            .and_then(|name| Tz::from_str(&name).ok())
            .unwrap_or(Tz::UTC)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const NOW: DateTime<Utc> = chrono::DateTime::<Utc>::UNIX_EPOCH; // 1970-01-01 (Thursday) UTC

    fn parse(raw: &str) -> Option<DateTime<Utc>> {
        parse_due_at_in(NOW, Tz::UTC, raw)
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0)
            .single()
            .expect("valid test instant")
    }

    #[test]
    fn rejects_empty_and_vague() {
        assert!(parse("").is_none());
        assert!(parse("next time").is_none());
        assert!(parse("次回").is_none());
        assert!(parse("someday").is_none());
    }

    #[test]
    fn parses_english_date_terms() {
        assert_eq!(parse("today"), Some(utc(1970, 1, 1, 23, 59)));
        assert_eq!(parse("tonight"), Some(utc(1970, 1, 1, 23, 59)));
        assert_eq!(parse("tomorrow"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("day after tomorrow"), Some(utc(1970, 1, 3, 23, 59)));
        assert_eq!(parse("next week"), Some(utc(1970, 1, 8, 23, 59)));
        assert_eq!(parse("next month"), Some(utc(1970, 2, 1, 23, 59)));
        assert_eq!(parse("next year"), Some(utc(1971, 1, 1, 23, 59)));
    }

    #[test]
    fn parses_japanese_date_terms() {
        assert_eq!(parse("今日"), Some(utc(1970, 1, 1, 23, 59)));
        assert_eq!(parse("今晩"), Some(utc(1970, 1, 1, 23, 59)));
        assert_eq!(parse("明日"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("明後日"), Some(utc(1970, 1, 3, 23, 59)));
        assert_eq!(parse("来週"), Some(utc(1970, 1, 8, 23, 59)));
        assert_eq!(parse("来月"), Some(utc(1970, 2, 1, 23, 59)));
        assert_eq!(parse("来年"), Some(utc(1971, 1, 1, 23, 59)));
    }

    #[test]
    fn parses_weekdays() {
        // 1970-01-01 is a Thursday.
        assert_eq!(parse("friday"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("thursday"), Some(utc(1970, 1, 1, 23, 59)));
        assert_eq!(parse("this friday"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("next friday"), Some(utc(1970, 1, 9, 23, 59)));
        assert_eq!(parse("next thursday"), Some(utc(1970, 1, 8, 23, 59)));
    }

    #[test]
    fn parses_japanese_weekdays() {
        assert_eq!(parse("金曜日"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("今週の金曜日"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("来週の金曜日"), Some(utc(1970, 1, 9, 23, 59)));
        assert_eq!(parse("次の木曜日"), Some(utc(1970, 1, 8, 23, 59)));
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse("in 3 days"), Some(utc(1970, 1, 4, 0, 0)));
        assert_eq!(parse("5 weeks from now"), Some(utc(1970, 2, 5, 0, 0)));
        assert_eq!(parse("in 2 hours"), Some(utc(1970, 1, 1, 2, 0)));
        assert_eq!(parse("in 30 minutes"), Some(utc(1970, 1, 1, 0, 30)));
        assert_eq!(parse("3日後"), Some(utc(1970, 1, 4, 0, 0)));
        assert_eq!(parse("2週間後"), Some(utc(1970, 1, 15, 0, 0)));
        assert_eq!(parse("5時間後"), Some(utc(1970, 1, 1, 5, 0)));
        assert_eq!(parse("90分後"), Some(utc(1970, 1, 1, 1, 30)));
        assert_eq!(parse("2か月後"), Some(utc(1970, 3, 1, 0, 0)));
        assert_eq!(parse("2ヶ月後"), Some(utc(1970, 3, 1, 0, 0)));
    }

    #[test]
    fn parses_english_times() {
        assert_eq!(parse("tomorrow at 3pm"), Some(utc(1970, 1, 2, 15, 0)));
        assert_eq!(parse("tomorrow 15:00"), Some(utc(1970, 1, 2, 15, 0)));
        assert_eq!(parse("tomorrow 9:30 am"), Some(utc(1970, 1, 2, 9, 30)));
        assert_eq!(parse("at 12am today"), Some(utc(1970, 1, 1, 0, 0)));
        assert_eq!(parse("12pm today"), Some(utc(1970, 1, 1, 12, 0)));
    }

    #[test]
    fn parses_japanese_times() {
        assert_eq!(parse("明日の15時"), Some(utc(1970, 1, 2, 15, 0)));
        assert_eq!(parse("明日の午後3時"), Some(utc(1970, 1, 2, 15, 0)));
        assert_eq!(parse("明日の午前9時半"), Some(utc(1970, 1, 2, 9, 30)));
        assert_eq!(parse("今晩午後9時"), Some(utc(1970, 1, 1, 21, 0)));
    }

    #[test]
    fn strips_prepositions() {
        assert_eq!(parse("by tomorrow"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("until friday"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("明日までに"), Some(utc(1970, 1, 2, 23, 59)));
        assert_eq!(parse("金曜日まで"), Some(utc(1970, 1, 2, 23, 59)));
    }

    #[test]
    fn month_shift_clamps_to_month_end() {
        // 1970-01-31 + 1 month → 1970-02-28.
        let jan31 = Utc
            .with_ymd_and_hms(1970, 1, 31, 12, 0, 0)
            .single()
            .unwrap();
        assert_eq!(
            parse_due_at_in(jan31, Tz::UTC, "next month"),
            Some(utc(1970, 2, 28, 23, 59))
        );
    }
}
