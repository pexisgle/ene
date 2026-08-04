//! Structured time-series world state memory for proactive speech.
//!
//! Snapshots of environment / user-activity signals are kept in a bounded
//! in-memory ring — never persisted — and analyzed into a trend summary that
//! feeds both the deterministic gates and the decision prompt.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::config::WorldStateConfig;
use crate::proactive::{ProactiveObservation, truncate_chars};

/// Label length cap applied when a snapshot is built from an observation.
const MAX_LABEL_CHARS: usize = 120;
/// Change-description length cap applied when a snapshot is built from an
/// observation.
const MAX_CHANGE_CHARS: usize = 160;

/// One structured world-state snapshot at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSnapshot {
    /// When the snapshot was captured (unix millis).
    pub captured_at_unix_ms: u64,
    /// Seconds since the last OS activity signal when the host measures it.
    pub idle_seconds: Option<u64>,
    /// Privacy-safe label of the focused window (app name, optionally a
    /// redacted title).
    pub active_window_label: String,
    /// Non-empty when the focused window changed since the previous snapshot.
    pub recent_change: String,
    /// Seconds since the last user message in the session at capture time.
    ///
    /// Part of the temporal context; not consumed by the current trend
    /// summary.
    pub seconds_since_user_input: u64,
}

impl WorldStateSnapshot {
    /// Build a snapshot from a host observation plus the session's user-input
    /// silence. Screen summaries are deliberately not stored: they are fresh
    /// per decision and more privacy-sensitive than window labels.
    #[must_use]
    pub fn from_observation(
        observation: &ProactiveObservation,
        seconds_since_user_input: u64,
    ) -> Self {
        let activity = observation.activity.as_ref();
        Self {
            captured_at_unix_ms: observation.captured_at_unix_ms,
            idle_seconds: activity.and_then(|a| a.idle_seconds),
            active_window_label: activity.map_or_else(String::new, |a| {
                truncate_chars(&a.active_window_label, MAX_LABEL_CHARS)
            }),
            recent_change: activity.map_or_else(String::new, |a| {
                truncate_chars(&a.recent_change, MAX_CHANGE_CHARS)
            }),
            seconds_since_user_input,
        }
    }
}

/// Direction of the idle trend across the ring, computed from the most recent
/// segment (latest vs. previous snapshot) when both carry a measured idle
/// value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleTrend {
    /// Latest idle is larger than the previous snapshot.
    Rising,
    /// Latest idle is smaller than the previous snapshot.
    Falling,
    /// Both values are present and equal.
    Steady,
    /// Idle is unmeasured on this host or no pair of values exists.
    #[default]
    Unknown,
}

/// Computed world-state analysis for one decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSummary {
    /// Idle direction in the latest snapshot pair (see [`IdleTrend`]).
    pub idle_trend: IdleTrend,
    /// Snapshots with a window switch inside the recent change window.
    pub window_changes: usize,
    /// True when the latest snapshot shows the user actively working.
    pub engaged: bool,
    /// Truncated privacy-safe label of the latest focused window.
    pub latest_window: String,
    /// Number of snapshots the analysis is based on.
    pub snapshot_count: usize,
}

/// Bounded in-memory ring of world-state snapshots, oldest first.
#[derive(Debug, Clone, Default)]
pub struct WorldStateMemory {
    snapshots: VecDeque<WorldStateSnapshot>,
}

impl WorldStateMemory {
    /// Append a snapshot, dropping the oldest once the configured capacity is
    /// reached. Snapshots are ordered by capture sequence; the host pushes one
    /// per observation interval.
    pub fn push(&mut self, snapshot: WorldStateSnapshot, config: &WorldStateConfig) {
        let cap = config.max_snapshots.max(1);
        if self.snapshots.len() >= cap {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
    }

    /// Drop all snapshots (session / character change).
    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    /// Number of retained snapshots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Whether the ring holds no snapshots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    /// The most recent snapshot, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&WorldStateSnapshot> {
        self.snapshots.back()
    }

    /// Snapshots in capture order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &WorldStateSnapshot> {
        self.snapshots.iter()
    }

    /// Compute the trend summary, or `None` when the feature is disabled or
    /// the ring holds fewer than `min_snapshots_for_trend` snapshots.
    #[must_use]
    pub fn summary(&self, config: &WorldStateConfig) -> Option<WorldStateSummary> {
        if !config.enabled || self.snapshots.len() < config.min_snapshots_for_trend.max(1) {
            return None;
        }
        let latest = self.snapshots.back()?;
        let previous = self.snapshots.iter().rev().nth(1).unwrap_or(latest);
        let idle_trend = match (previous.idle_seconds, latest.idle_seconds) {
            (Some(previous), Some(latest)) if latest > previous => IdleTrend::Rising,
            (Some(previous), Some(latest)) if latest < previous => IdleTrend::Falling,
            (Some(_), Some(_)) => IdleTrend::Steady,
            _ => IdleTrend::Unknown,
        };
        let window = config.change_window.max(1);
        let window_changes = self
            .snapshots
            .iter()
            .rev()
            .take(window)
            .filter(|s| !s.recent_change.is_empty())
            .count();
        let engaged = !latest.recent_change.is_empty()
            || latest
                .idle_seconds
                .is_some_and(|idle| idle < config.engaged_idle_seconds);
        Some(WorldStateSummary {
            idle_trend,
            window_changes,
            engaged,
            latest_window: truncate_chars(&latest.active_window_label, MAX_LABEL_CHARS),
            snapshot_count: self.snapshots.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proactive::{ActivitySnapshot, ScreenSummaryStatus};

    fn config() -> WorldStateConfig {
        WorldStateConfig {
            enabled: true,
            ..WorldStateConfig::default()
        }
    }

    fn snapshot(captured_at: u64, idle: Option<u64>, change: &str) -> WorldStateSnapshot {
        WorldStateSnapshot {
            captured_at_unix_ms: captured_at,
            idle_seconds: idle,
            active_window_label: "Code".into(),
            recent_change: change.into(),
            seconds_since_user_input: 60,
        }
    }

    #[test]
    fn ring_drops_oldest_at_capacity() {
        let mut ring = WorldStateMemory::default();
        let cfg = WorldStateConfig {
            enabled: true,
            max_snapshots: 2,
            ..WorldStateConfig::default()
        };
        ring.push(snapshot(1, None, ""), &cfg);
        ring.push(snapshot(2, None, ""), &cfg);
        ring.push(snapshot(3, None, ""), &cfg);
        assert_eq!(ring.len(), 2);
        assert_eq!(ring.latest().expect("latest").captured_at_unix_ms, 3);
        assert_eq!(ring.iter().next().expect("oldest").captured_at_unix_ms, 2);
    }

    #[test]
    fn summary_is_none_when_disabled_or_below_min_snapshots() {
        let mut ring = WorldStateMemory::default();
        assert_eq!(ring.summary(&config()), None);

        ring.push(snapshot(1, Some(10), ""), &config());
        ring.push(snapshot(2, Some(20), ""), &config());
        assert_eq!(ring.summary(&config()), None);

        ring.push(snapshot(3, Some(30), ""), &config());
        assert_eq!(ring.summary(&config()).expect("summary").snapshot_count, 3);

        let disabled = WorldStateConfig {
            enabled: false,
            ..WorldStateConfig::default()
        };
        assert_eq!(ring.summary(&disabled), None);
    }

    #[test]
    fn idle_trend_uses_the_latest_segment() {
        let mut ring = WorldStateMemory::default();
        for (i, idle) in [Some(10u64), Some(20), Some(30)].into_iter().enumerate() {
            ring.push(snapshot(i as u64 + 1, idle, ""), &config());
        }
        assert_eq!(
            ring.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Rising
        );

        ring.clear();
        for (i, idle) in [Some(30u64), Some(20), Some(10)].into_iter().enumerate() {
            ring.push(snapshot(i as u64 + 1, idle, ""), &config());
        }
        assert_eq!(
            ring.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Falling
        );

        ring.clear();
        for (i, idle) in [Some(20u64), Some(20), Some(20)].into_iter().enumerate() {
            ring.push(snapshot(i as u64 + 1, idle, ""), &config());
        }
        assert_eq!(
            ring.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Steady
        );
    }

    #[test]
    fn non_monotonic_idle_series_follows_the_latest_segment() {
        let mut ring = WorldStateMemory::default();
        // The overall series rises (60 → 120 → 90) but the most recent
        // segment falls: the user is returning toward activity, so the trend
        // must report Falling rather than Rising.
        for (i, idle) in [Some(60u64), Some(120), Some(90)].into_iter().enumerate() {
            ring.push(snapshot(i as u64 + 1, idle, ""), &config());
        }
        assert_eq!(
            ring.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Falling
        );
    }

    #[test]
    fn idle_trend_is_unknown_without_measured_idle() {
        let mut ring = WorldStateMemory::default();
        for i in 0..3 {
            ring.push(snapshot(i as u64 + 1, None, ""), &config());
        }
        assert_eq!(
            ring.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Unknown
        );

        // The latest segment carries a missing value, so the direction is
        // unknown regardless of older measurements.
        let mixed = WorldStateMemory {
            snapshots: VecDeque::from([
                snapshot(1, Some(10), ""),
                snapshot(2, None, ""),
                snapshot(3, Some(20), ""),
            ]),
        };
        assert_eq!(
            mixed.summary(&config()).expect("summary").idle_trend,
            IdleTrend::Unknown
        );
    }

    #[test]
    fn window_changes_counts_only_the_recent_window() {
        let mut ring = WorldStateMemory::default();
        let cfg = WorldStateConfig {
            enabled: true,
            change_window: 2,
            ..WorldStateConfig::default()
        };
        ring.push(snapshot(1, None, "switched"), &cfg);
        ring.push(snapshot(2, None, ""), &cfg);
        ring.push(snapshot(3, None, "switched"), &cfg);
        assert_eq!(ring.summary(&cfg).expect("summary").window_changes, 1);
    }

    #[test]
    fn engaged_on_window_switch_or_low_idle() {
        let mut ring = WorldStateMemory::default();
        let cfg = WorldStateConfig {
            enabled: true,
            engaged_idle_seconds: 60,
            ..WorldStateConfig::default()
        };
        ring.push(snapshot(1, Some(120), ""), &cfg);
        ring.push(snapshot(2, Some(120), ""), &cfg);
        ring.push(snapshot(3, Some(30), ""), &cfg);
        assert!(ring.summary(&cfg).expect("summary").engaged);

        ring.clear();
        ring.push(snapshot(1, Some(120), ""), &cfg);
        ring.push(snapshot(2, Some(120), ""), &cfg);
        ring.push(snapshot(3, Some(120), "switched"), &cfg);
        assert!(ring.summary(&cfg).expect("summary").engaged);
    }

    #[test]
    fn not_engaged_when_idle_unknown_and_no_switch() {
        let mut ring = WorldStateMemory::default();
        for i in 0..3 {
            ring.push(snapshot(i as u64 + 1, None, ""), &config());
        }
        let summary = ring.summary(&config()).expect("summary");
        assert!(!summary.engaged);
        assert_eq!(summary.window_changes, 0);
    }

    #[test]
    fn snapshot_from_observation_maps_fields_and_truncates() {
        let observation = ProactiveObservation {
            captured_at_unix_ms: 42,
            activity: Some(ActivitySnapshot {
                idle_seconds: Some(7),
                active_window_label: "x".repeat(300),
                recent_change: "y".repeat(400),
            }),
            screen_summary: Some("never stored".into()),
            screen_summary_status: ScreenSummaryStatus::Available,
        };
        let snap = WorldStateSnapshot::from_observation(&observation, 90);
        assert_eq!(snap.captured_at_unix_ms, 42);
        assert_eq!(snap.idle_seconds, Some(7));
        assert_eq!(snap.active_window_label.chars().count(), MAX_LABEL_CHARS);
        assert_eq!(snap.recent_change.chars().count(), MAX_CHANGE_CHARS);
        assert_eq!(snap.seconds_since_user_input, 90);

        let no_activity = WorldStateSnapshot::from_observation(&ProactiveObservation::default(), 0);
        assert_eq!(no_activity.captured_at_unix_ms, 0);
        assert_eq!(no_activity.idle_seconds, None);
        assert!(no_activity.active_window_label.is_empty());
    }
}
