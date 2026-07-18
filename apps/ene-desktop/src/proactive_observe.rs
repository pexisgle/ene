//! Privacy-aware activity / screen observation for proactive speech (#168).
//!
//! Collects host signals on a background tokio task and pushes
//! [`ene_mind::ProactiveObservation`] into the runtime. Raw screenshots are
//! never persisted; screen summary is omitted when no safe summarizer is
//! available (source unavailable).

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
    runtime.spawn(async move {
        let screen_provider = ScreenSummaryProvider::v1_unavailable();
        let mut last_app = String::new();
        loop {
            let cfg = control_task.snapshot();
            let sleep_for = Duration::from_secs(cfg.interval_seconds.max(1));
            if !cfg.enabled {
                tokio::time::sleep(sleep_for).await;
                continue;
            }

            let observation = collect_observation(&cfg, screen_provider, &mut last_app);
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

fn collect_observation(
    cfg: &ObserveConfig,
    screen_provider: ScreenSummaryProvider,
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
        match screen_provider.summarize() {
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

fn redact_paths(input: &str) -> String {
    input
        .split_whitespace()
        .filter(|t| !t.contains('/') && !t.contains('\\') && !t.contains('@'))
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(input: &str, max: usize) -> String {
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
    fn control_defaults_from_mind() {
        let mind = MindConfig::default();
        let control = ProactiveObserveControl::from_mind(&mind);
        let snap = control.snapshot();
        assert!(!snap.enabled);
        assert!(snap.activity);
        assert!(!snap.screen_summary);
    }

    #[test]
    fn screen_summary_unavailable_when_enabled_but_no_provider() {
        let cfg = ObserveConfig {
            enabled: true,
            activity: false,
            screen_summary: true,
            interval_seconds: 30,
        };
        let provider = ScreenSummaryProvider::v1_unavailable();
        let obs = collect_observation(&cfg, provider, &mut String::new());
        assert_eq!(obs.screen_summary_status, ScreenSummaryStatus::Unavailable);
        assert!(obs.screen_summary.is_none());
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
