//! Privacy-aware activity / screen observation for proactive speech.
//!
//! Collects host signals on a background tokio task and pushes
//! [`ene_mind::ProactiveObservation`] into the runtime. Raw screenshots are
//! never persisted; only a short text summary crosses the mind boundary.
//! Screen pixels are summarized in-process by the local Gemma + mmproj model.

mod capture;
mod diff_gate;
mod ocr;
mod roi;
mod screen_summary;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ene_mind::{
    ActivitySnapshot, MindConfig, ProactiveObservation, ScreenSummaryStatus, WindowTitleLevel,
};
use ene_runtime::EneHandle;
use parking_lot::Mutex;

use screen_summary::ScreenSummaryProvider;

/// Shared knobs so settings UI can pause / retarget observation without
/// restarting the desktop process.
#[derive(Debug, Clone)]
pub struct ProactiveObserveControl {
    inner: Arc<Mutex<ObserveConfig>>,
}

#[derive(Debug, Clone)]
struct ObserveConfig {
    enabled: bool,
    activity: bool,
    screen_summary: bool,
    interval_seconds: u64,
    max_screen_summary_chars: usize,
    window_title_level: WindowTitleLevel,
}

impl ProactiveObserveControl {
    #[must_use]
    pub fn from_mind(mind: &MindConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ObserveConfig {
                enabled: mind.proactive.enabled,
                activity: mind.proactive.sources.activity,
                screen_summary: mind.proactive.sources.screen_summary,
                interval_seconds: mind.proactive.interval_seconds.max(1),
                max_screen_summary_chars: mind.proactive.max_screen_summary_chars.max(32),
                window_title_level: mind.proactive.sources.window_title_level,
            })),
        }
    }

    pub fn apply_mind(&self, mind: &MindConfig) {
        let mut guard = self.inner.lock();
        guard.enabled = mind.proactive.enabled;
        guard.activity = mind.proactive.sources.activity;
        guard.screen_summary = mind.proactive.sources.screen_summary;
        guard.interval_seconds = mind.proactive.interval_seconds.max(1);
        guard.max_screen_summary_chars = mind.proactive.max_screen_summary_chars.max(32);
        guard.window_title_level = mind.proactive.sources.window_title_level;
    }

    fn snapshot(&self) -> ObserveConfig {
        self.inner.lock().clone()
    }
}

/// Spawn the observation loop. Returns the control handle.
pub fn spawn_proactive_observer(
    runtime: &tokio::runtime::Handle,
    handle: EneHandle,
    mind: &MindConfig,
) -> ProactiveObserveControl {
    let control = ProactiveObserveControl::from_mind(mind);
    let control_task = control.clone();
    let observe_handle = handle.clone();
    runtime.spawn(async move {
        let screen_provider = ScreenSummaryProvider::new(observe_handle.clone());
        let mut last_label = String::new();
        loop {
            let cfg = control_task.snapshot();
            let sleep_for = Duration::from_secs(cfg.interval_seconds.max(1));
            if !cfg.enabled {
                tokio::time::sleep(sleep_for).await;
                continue;
            }

            let observation = collect_observation(&cfg, &screen_provider, &mut last_label).await;
            if let Err(e) = handle.update_proactive_observation(observation) {
                tracing::debug!(
                    component = "ProactiveObserve",
                    error = %e,
                    "Failed to push observation (actor dead?)"
                );
                break;
            }
            tokio::time::sleep(sleep_for).await;
        }
    });
    control
}

async fn collect_observation(
    cfg: &ObserveConfig,
    screen_provider: &ScreenSummaryProvider,
    last_label: &mut String,
) -> ProactiveObservation {
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);

    let activity = if cfg.activity {
        Some(collect_activity(cfg.window_title_level, last_label))
    } else {
        None
    };

    let (screen_summary, screen_summary_status) = if cfg.screen_summary {
        let cursor = current_cursor_position();
        match screen_provider
            .summarize(cfg.max_screen_summary_chars, cursor)
            .await
        {
            Some(text) => (Some(text), ScreenSummaryStatus::Available),
            None => (None, ScreenSummaryStatus::Unavailable),
        }
    } else {
        (None, ScreenSummaryStatus::Disabled)
    };

    ProactiveObservation {
        captured_at_unix_ms,
        activity,
        screen_summary,
        screen_summary_status,
    }
}

fn collect_activity(level: WindowTitleLevel, last_label: &mut String) -> ActivitySnapshot {
    match active_window_label(level) {
        Some(label) => {
            let recent_change = describe_change(last_label, &label);
            ActivitySnapshot {
                idle_seconds: None,
                active_window_label: truncate(&label, 120),
                recent_change: truncate(&recent_change, 160),
            }
        }
        None => ActivitySnapshot {
            idle_seconds: None,
            active_window_label: "unavailable".into(),
            recent_change: String::new(),
        },
    }
}

/// Summarize the change from the previous observation's label to the current
/// one. Returns an empty string when nothing changed so the decision
/// prompt is not padded with noise; otherwise a short phrase naming the
/// previous and/or current focus.
fn describe_change(last_label: &mut String, label: &str) -> String {
    if last_label == label {
        return String::new();
    }
    let change = if last_label.is_empty() {
        format!("focused {label}")
    } else {
        format!("switched from {last_label} to {label}")
    };
    label.clone_into(last_label);
    change
}

/// Build the privacy-aware label for the focused window at `level`.
///
/// Returns `None` when no window is focused or the app name is empty. The app
/// name is always included; the window title is appended only at
/// [`WindowTitleLevel::RedactedTitle`] (sanitized) or
/// [`WindowTitleLevel::FullTitle`] (verbatim).
fn active_window_label(level: WindowTitleLevel) -> Option<String> {
    let win = active_win_pos_rs::get_active_window().ok()?;
    let app = redact_paths(win.app_name.trim());
    if app.is_empty() {
        return None;
    }
    let title = match level {
        WindowTitleLevel::AppOnly => return Some(app),
        WindowTitleLevel::RedactedTitle => redact_title(win.title.trim()),
        WindowTitleLevel::FullTitle => collapse_whitespace(win.title.trim()),
    };
    if title.is_empty() {
        Some(app)
    } else {
        Some(format!("{app}: {title}"))
    }
}

/// Read the current global cursor position (screen coordinates) for
/// ROI cropping. Returns `None` when the device state cannot be
/// queried, in which case the observer skips ROI cropping.
fn current_cursor_position() -> Option<(i32, i32)> {
    use device_query::{DeviceQuery, DeviceState};
    let state = DeviceState::new();
    let mouse = state.get_mouse();
    Some(mouse.coords)
}

pub(crate) fn redact_paths(input: &str) -> String {
    input
        .split_whitespace()
        .filter(|t| !looks_like_path_or_email(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Redact a window title for [`WindowTitleLevel::RedactedTitle`].
///
/// Reuses the path / email filter from [`redact_paths`] and additionally drops
/// URL-like tokens and digit-heavy numbers (phone / account / card numbers),
/// then strips long interior digit runs from the tokens that are kept.
/// Standalone document names such as `顧客リスト.xlsx` are intentionally
/// preserved — only the surrounding path is sensitive. The result may be empty
/// when every token looked sensitive.
fn redact_title(input: &str) -> String {
    input
        .split_whitespace()
        .filter(|t| !looks_like_path_or_email(t) && !looks_like_url(t) && !is_digit_heavy(t))
        .map(strip_digit_runs)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collapse interior runs of whitespace to single ASCII spaces so a raw title
/// ([`WindowTitleLevel::FullTitle`]) cannot smuggle control characters or
/// padded layout into the decision prompt.
fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Returns `true` when a token is unambiguously a URL: it carries an explicit
/// scheme (`https://…`, `file://…`) or starts with `www.`.
///
/// Bare domains (`example.com`) are deliberately *not* matched — they are
/// indistinguishable from document filenames (`report.docx`) at the token
/// level, and dropping filenames would defeat the purpose of the
/// [`WindowTitleLevel::RedactedTitle`] level.
fn looks_like_url(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    lower.contains("://") || lower.starts_with("www.")
}

/// Returns `true` when a token is dominated by digits — at least 6 digit
/// characters making up more than half the token — the shape of phone,
/// account, and card numbers that carry no topical signal.
fn is_digit_heavy(t: &str) -> bool {
    let digits = t.chars().filter(char::is_ascii_digit).count();
    digits >= 6 && digits * 2 > t.chars().count()
}

/// Remove runs of 4+ ASCII digits from a token (e.g. `report2024final` →
/// `reportfinal`, `call 03-1234-5678` fragments), while keeping short ordinals
/// like `v2` or `page3` intact.
fn strip_digit_runs(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut digits = String::new();
    for c in t.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            if digits.len() < 4 {
                out.push_str(&digits);
            }
            digits.clear();
            out.push(c);
        }
    }
    if digits.len() < 4 {
        out.push_str(&digits);
    }
    out
}

/// Returns `true` when a token is likely a filesystem path or email
/// address rather than a meaningful application/window-title word.
///
/// Heuristic: a token is path-like when it contains a path separator
/// AND at least one of:
/// - starts with `/`, `\`, or `~` (absolute / home paths)
/// - contains a drive-letter prefix (`X:/` or `X:\`)
/// - has two or more separators (multi-component relative paths)
/// - ends with a file extension (e.g. `docs/report.md`)
///
/// A single interior slash with no extension (e.g. `and/or`) is kept.
fn looks_like_path_or_email(t: &str) -> bool {
    if t.contains('@') && t.contains('.') {
        return true;
    }
    let sep_count = t.chars().filter(|c| *c == '/' || *c == '\\').count();
    if sep_count == 0 {
        return false;
    }
    if t.starts_with('/') || t.starts_with('\\') || t.starts_with('~') {
        return true;
    }
    if t.len() >= 3 {
        let bytes = t.as_bytes();
        if bytes[1] == b':' && (bytes[2] == b'/' || bytes[2] == b'\\') {
            return true;
        }
    }
    if sep_count >= 2 {
        return true;
    }
    has_file_extension(t)
}

/// Returns `true` when the token ends with a short file extension
/// (1–5 alphanumeric chars after the last `.`), e.g. `report.md`.
fn has_file_extension(t: &str) -> bool {
    let Some(dot) = t.rfind('.') else {
        return false;
    };
    let ext = &t[dot + 1..];
    !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

pub(crate) fn truncate(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        input.to_string()
    } else {
        input.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_drops_path_like_tokens() {
        let cleaned = redact_paths("Editor /home/user/secret.rs note");
        assert!(!cleaned.contains("/home"));
        assert!(cleaned.contains("Editor"));
        assert!(cleaned.contains("note"));
    }

    #[test]
    fn redact_keeps_single_slash_words() {
        // A single interior slash is not a path (e.g. "and/or").
        let cleaned = redact_paths("and/or maybe");
        assert!(cleaned.contains("and/or"));
        assert!(cleaned.contains("maybe"));
    }

    #[test]
    fn redact_drops_drive_and_relative_paths() {
        let cleaned = redact_paths("C:\\Users\\me docs/report.md app");
        assert!(!cleaned.contains("C:\\"));
        assert!(!cleaned.contains("docs/report.md"));
        assert!(cleaned.contains("app"));
    }

    #[test]
    fn redact_drops_email() {
        let cleaned = redact_paths("contact me@example.com now");
        assert!(!cleaned.contains("me@example.com"));
        assert!(cleaned.contains("contact"));
        assert!(cleaned.contains("now"));
    }

    #[test]
    fn control_defaults_from_mind() {
        let mind = MindConfig::default();
        let control = ProactiveObserveControl::from_mind(&mind);
        let snap = control.snapshot();
        assert!(!snap.enabled);
        assert!(snap.activity);
        assert!(!snap.screen_summary);
        assert_eq!(snap.window_title_level, WindowTitleLevel::AppOnly);
    }

    #[test]
    fn redact_title_drops_sensitive_tokens() {
        let cleaned = redact_title(
            "Inbox me@example.com https://mail.example.com/a C:\\Users\\me report 0312345678",
        );
        assert!(!cleaned.contains("me@example.com"));
        assert!(!cleaned.contains("https://"));
        assert!(!cleaned.contains("C:\\"));
        assert!(!cleaned.contains("0312345678"));
        assert!(cleaned.contains("Inbox"));
        assert!(cleaned.contains("report"));
    }

    #[test]
    fn redact_title_preserves_document_names() {
        // The motivating case: a standalone document name carries topical
        // signal and must survive redaction (only the path around it is
        // sensitive).
        let cleaned = redact_title("顧客リスト.xlsx - Excel");
        assert!(cleaned.contains("顧客リスト.xlsx"));
        assert!(cleaned.contains("Excel"));

        let cleaned = redact_title("report.docx - Word");
        assert!(cleaned.contains("report.docx"));
    }

    #[test]
    fn redact_title_strips_long_digit_runs_only() {
        // Long interior runs (years, ids) are stripped; short ordinals kept.
        let cleaned = redact_title("report2024final v2 page3");
        assert!(cleaned.contains("reportfinal"));
        assert!(cleaned.contains("v2"));
        assert!(cleaned.contains("page3"));
    }

    #[test]
    fn looks_like_url_matches_scheme_and_www_only() {
        assert!(looks_like_url("https://example.com/page"));
        assert!(looks_like_url("www.example.com"));
        // Bare domains are indistinguishable from filenames and must not match.
        assert!(!looks_like_url("report.docx"));
        assert!(!looks_like_url("example.com"));
    }

    #[test]
    fn is_digit_heavy_flags_numbers_only() {
        assert!(is_digit_heavy("0312345678"));
        assert!(is_digit_heavy("4111-1111-2222"));
        assert!(!is_digit_heavy("v2"));
        assert!(!is_digit_heavy("report2024"));
    }

    #[test]
    fn collapse_whitespace_normalizes_runs() {
        assert_eq!(collapse_whitespace("a\t b\n\n c"), "a b c");
    }

    #[test]
    fn describe_change_tracks_focus_and_switches() {
        let mut last = String::new();
        assert_eq!(describe_change(&mut last, "firefox"), "focused firefox");
        assert_eq!(last, "firefox");

        // No change → empty string.
        assert!(describe_change(&mut last, "firefox").is_empty());

        // Switch names both the previous and current focus.
        assert_eq!(
            describe_change(&mut last, "code: main.rs"),
            "switched from firefox to code: main.rs"
        );
        assert_eq!(last, "code: main.rs");
    }
}
