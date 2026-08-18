//! Beat-sync shared state.
//!
//! [`BeatSyncState`] holds the pulses relayed from the runtime chat bus and
//! the enabled flag; the render loop drains it once per frame to drive the
//! avatar's procedural sway. [`BeatSyncRuntime`] owns the capture-thread
//! handle so the Features-page toggle can start/stop capture live.

use std::collections::VecDeque;

use bevy_ecs::prelude::*;

use crate::event::ai::BeatPulse;

/// Maximum pending pulses kept for the render loop; at ≤4 pulses/s this
/// covers many stalled frames before the oldest pulse is dropped.
const PULSE_QUEUE_CAPACITY: usize = 32;

/// A detected beat normalized for the avatar layer.
#[derive(Debug, Clone, Copy)]
pub struct BeatPulseSnapshot {
    /// Estimated tempo in beats per minute.
    pub bpm: f32,
    /// Normalized onset strength in `[0, 1]`.
    pub intensity: f32,
}

#[derive(Resource, Default)]
pub struct BeatSyncState {
    enabled: bool,
    pulses: VecDeque<BeatPulseSnapshot>,
}

impl BeatSyncState {
    /// Set whether beat sync is active; disabling drops pending pulses.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.pulses.clear();
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Queue a pulse for the render loop; ignored while disabled.
    pub fn push_pulse(&mut self, pulse: BeatPulseSnapshot) {
        if self.enabled {
            if self.pulses.len() >= PULSE_QUEUE_CAPACITY {
                self.pulses.pop_front();
            }
            self.pulses.push_back(pulse);
        }
    }

    pub fn drain_pulses(&mut self) -> impl Iterator<Item = BeatPulseSnapshot> + '_ {
        self.pulses.drain(..)
    }
}

pub fn apply_beat_pulses_system(
    mut events: MessageReader<BeatPulse>,
    mut state: ResMut<BeatSyncState>,
) {
    for pulse in events.read() {
        state.push_pulse(BeatPulseSnapshot {
            bpm: pulse.bpm,
            intensity: pulse.intensity,
        });
    }
}

/// Owns the beat-sync capture thread; `None` while capture is stopped.
///
/// Unlike the `!Send` microphone stream, the capture handle is `Send`, so
/// it can live in the ECS world and the Features-page toggle can start and
/// stop it without a chat-UI ownership hack.
#[cfg(feature = "voice")]
#[derive(Resource, Default)]
pub struct BeatSyncRuntime(Option<crate::audio::beat_sync::BeatSyncHandle>);

#[cfg(feature = "voice")]
impl BeatSyncRuntime {
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(crate::audio::beat_sync::BeatSyncHandle::is_alive)
    }

    pub fn stop(&mut self) {
        if let Some(mut handle) = self.0.take() {
            handle.stop();
        }
    }

    pub fn replace(&mut self, handle: crate::audio::beat_sync::BeatSyncHandle) {
        self.stop();
        self.0 = Some(handle);
    }
}

#[cfg(feature = "voice")]
impl Drop for BeatSyncRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}
