use crate::config::WorldStateSettings;
use crate::proactive::{ProactiveObservation, truncate_chars};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const MAX_LABEL_CHARS: usize = 120;
const MAX_CHANGE_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSnapshot {
    pub captured_at_unix_ms: u64,
    pub idle_seconds: Option<u64>,
    pub active_window_label: String,
    pub recent_change: String,
    pub seconds_since_user_input: u64,
}

impl WorldStateSnapshot {
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdleTrend {
    Rising,
    Falling,
    Steady,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSummary {
    pub idle_trend: IdleTrend,
    pub window_changes: usize,
    pub engaged: bool,
    pub latest_window: String,
    pub snapshot_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct WorldStateMemory {
    snapshots: VecDeque<WorldStateSnapshot>,
}

impl WorldStateMemory {
    pub fn push(&mut self, snapshot: WorldStateSnapshot, config: &WorldStateSettings) {
        let cap = config.max_snapshots.max(1);
        if self.snapshots.len() >= cap {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
    }

    #[must_use]
    pub fn summary(&self, config: &WorldStateSettings) -> Option<WorldStateSummary> {
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
