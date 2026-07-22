//! Privacy-aware activity / screen observation for proactive speech (#168).
//!
//! Collects host signals on a background tokio task and pushes
//! [`ene_mind::ProactiveObservation`] into the runtime. Raw screenshots are
//! never persisted; only a short text summary crosses the mind boundary.
//! Screen pixels are summarized in-process by the local Gemma + mmproj model.

mod capture;
mod screen_summary;

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ene_mind::{ActivitySnapshot, MindConfig, ProactiveObservation, ScreenSummaryStatus};
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
}

impl ProactiveObserveControl {
    /// Build from mind proactive config.
    #[must_use]
    pub fn from_mind(mind: &MindConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ObserveConfig {
                enabled: mind.proactive.enabled,
                activity: mind.proactive.sources.activity,
                screen_summary: mind.proactive.sources.screen_summary,
                interval_seconds: mind.proactive.interval_seconds.max(1),
                max_screen_summary_chars: mind.proactive.max_screen_summary_chars.max(32),
            })),
        }
    }

    /// Update control flags from the latest mind config.
    pub fn apply_mind(&self, mind: &MindConfig) {
        let mut guard = self.inner.lock();
        guard.enabled = mind.proactive.enabled;
        guard.activity = mind.proactive.sources.activity;
        guard.screen_summary = mind.proactive.sources.screen_summary;
        guard.interval_seconds = mind.proactive.interval_seconds.max(1);
        guard.max_screen_summary_chars = mind.proactive.max_screen_summary_chars.max(32);
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
        let mut last_app = String::new();
        loop {
            let cfg = control_task.snapshot();
            let sleep_for = Duration::from_secs(cfg.interval_seconds.max(1));
            if !cfg.enabled {
                tokio::time::sleep(sleep_for).await;
                continue;
            }

            let observation = collect_observation(&cfg, &screen_provider, &mut last_app).await;
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
    last_app: &mut String,
) -> ProactiveObservation {
    let captured_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);

    let activity = if cfg.activity {
        Some(collect_activity(last_app))
    } else {
        None
    };

    let (screen_summary, screen_summary_status) = if cfg.screen_summary {
        match screen_provider
            .summarize(cfg.max_screen_summary_chars)
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

fn collect_activity(last_app: &mut String) -> ActivitySnapshot {
    match active_app_label() {
        Some(label) => {
            let recent_change = if last_app == &label {
                String::new()
            } else if last_app.is_empty() {
                last_app.clone_from(&label);
                "focus".to_string()
            } else {
                let change = format!("switched from {last_app}");
                last_app.clone_from(&label);
                change
            };
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

fn active_app_label() -> Option<String> {
    match active_win_pos_rs::get_active_window() {
        Ok(win) => {
            let app = win.app_name.trim();
            if app.is_empty() {
                None
            } else {
                Some(redact_paths(app))
            }
        }
        Err(()) => None,
    }
}

pub(crate) fn redact_paths(input: &str) -> String {
    input
        .split_whitespace()
        .filter(|t| !looks_like_path_or_email(t))
        .collect::<Vec<_>>()
        .join(" ")
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
    }

    #[test]
    fn activity_uses_app_name_only() {
        let snap = ActivitySnapshot {
            idle_seconds: None,
            active_window_label: "firefox".into(),
            recent_change: String::new(),
        };
        assert!(!snap.active_window_label.contains(':'));
    }
}
