//! Per-frame timing state.
use std::time::Instant;

use bevy_ecs::prelude::*;

/// Per-frame timing snapshot updated by the `First`-stage frame system
/// (added in Phase 2; for now the field is read by
/// [`crate::runtime::Runtime::about_to_wait`] once we wire the
/// schedule output back into the legacy state).
///
/// The fields are read by any system that needs `delta` (animation
/// blending, drag integration, FPS counter) or `elapsed` (AI bridge
/// timeout, emotion queue scheduling).
#[derive(Resource, Debug, Default)]
#[expect(dead_code, reason = "yet to be wired to frame systems")]
pub struct FrameState {
    /// Wall-clock time of the previous tick, used to compute `delta`.
    last_instant: Option<Instant>,
    /// Seconds elapsed since the previous tick, clamped to `[0, 0.1]`.
    pub delta: f32,
    /// Total seconds since the schedule started running.
    pub elapsed: f64,
    /// Monotonic frame counter, zero-indexed.
    pub frame_id: u64,
}

impl FrameState {
    /// Returns the previous tick's [`Instant`], if any.
    #[expect(dead_code, reason = "yet to be wired to frame system")]
    pub const fn last_instant(&self) -> Option<Instant> {
        self.last_instant
    }

    /// Records a new `last_instant` value. Called by the frame system
    /// added in Phase 2.
    #[expect(dead_code, reason = "yet to be wired to frame system")]
    pub const fn set_last_instant(&mut self, instant: Instant) {
        self.last_instant = Some(instant);
    }
}
