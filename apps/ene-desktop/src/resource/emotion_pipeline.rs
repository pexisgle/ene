//! Emotion pipeline state.
//!
//! The [`apply_emotions_system`](crate::system::ui_consumers::apply_emotions_system)
//! drains the per-entity [`UiEmotionQueue`](crate::component::ui::UiEmotionQueue)
//! into this resource every frame. The character-render path reads
//! the queue (and tracks the active emotion) instead of
//! `AppState`-side scratch state.
//!
//! Phase 7.5: replaces the legacy `PendingActions::emotion_commands`
//! buffer. Phase 7.2 wires the renderer call into a system that
//! reads this resource directly. The current implementation
//! drains the queue here and the winit driver applies the result
//! to the VRM model on every redraw.
use std::collections::VecDeque;

use bevy_ecs::prelude::*;

use crate::character_state::{ActiveEmotion, EmotionCommand};

#[derive(Resource, Debug)]
pub struct EmotionPipelineState {
    /// Commands buffered by the AI bridge / settings UI. Drained by
    /// the render-side system in `AppSet::Render`.
    pub pending: VecDeque<EmotionCommand>,
    /// Last-applied emotion + remaining hold time. Read by the
    /// render-side system to drive fade-out after the hold window.
    pub active: Option<ActiveEmotion>,
    /// Hold duration for new expression commands (`mind.emotion.expression_hysteresis_seconds`).
    pub expression_hold_secs: f64,
}

impl Default for EmotionPipelineState {
    fn default() -> Self {
        Self {
            pending: VecDeque::new(),
            active: None,
            expression_hold_secs: 4.0,
        }
    }
}

/// How long the weight takes to ramp from target -> 0 once the
/// hold window expires. The ramp-in is instant — when a command
/// pops, its `weight` is the new active weight.
pub const FADE_SECS: f64 = 0.3;

/// The result of a [`tick_emotions`] call: the renderer should
/// apply `name` with `weight` to the `VrmModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedEmotion {
    /// Expression name to set on the VRM model. Empty string
    /// means "no expression" (cleared).
    pub name: String,
    /// Weight in `[0, 1]`. `0.0` means "clear".
    pub weight: f32,
}

/// Tick the emotion pipeline: pop newly-arrived commands, track
/// the active emotion, and compute the current weight using a
/// hold + fade schedule.
///
/// `now_secs` is the current wall-clock seconds-since-epoch as
/// used by every producer of [`EmotionCommand`]
/// (`page_character.rs`, widgets.rs, the AI bridge). Keeping the
/// units on the producer side means the only clock the producers
/// and consumer must agree on is "monotonic seconds since
/// startup", which is trivial to provide via `Instant::now()`.
///
/// Algorithm (single active emotion, last write wins on a name
/// change):
///
/// 1. Pop the next `EmotionCommand` if `now_secs >= target_time`.
///    The popped command becomes the new `active` with
///    `hold_until_secs = target_time + hold_secs` and
///    `weight` = the command's target weight.
/// 2. If no command arrived, advance the active emotion's
///    weight:
///     * during the hold window (`now_secs < hold_until_secs`):
///       keep `weight` at the command's target.
///     * during the fade-out (`hold_until_secs <= now_secs <
///       hold_until_secs + FADE_SECS`): ramp linearly from
///       target to 0.
///     * after fade: `active` is cleared and the function
///       returns `weight = 0.0`.
pub fn tick_emotions(state: &mut EmotionPipelineState, now_secs: f64) -> AppliedEmotion {
    let mut result = AppliedEmotion {
        name: String::new(),
        weight: 0.0,
    };

    // 1. Pop the next command whose target_time has elapsed.
    loop {
        // Borrow the front to inspect the target time, then pop and
        // process it. The `front` borrow must end before `pop_front`
        // (which needs `&mut self`) can run.
        let target_time = match state.pending.front() {
            Some(front) if front.target_time <= now_secs => front.target_time,
            _ => break,
        };
        let cmd = state.pending.pop_front();
        let Some(cmd) = cmd else {
            break;
        };
        state.active = Some(ActiveEmotion {
            name: cmd.emotion.clone(),
            weight: cmd.weight,
            hold_until_secs: target_time + cmd.hold_secs,
        });
    }

    // 2. Tick the active emotion's weight.
    let Some(active) = state.active.as_ref() else {
        return result;
    };
    let target = active.weight;
    let name = active.name.clone();
    let hold_until_secs = active.hold_until_secs;

    if now_secs < hold_until_secs {
        // Still holding. Weight is at target.
        result.name = name;
        result.weight = target;
    } else {
        // Hold window expired — fade out over FADE_SECS.
        let fade_elapsed = now_secs - hold_until_secs;
        if fade_elapsed >= FADE_SECS {
            // Done. Clear and return 0.0.
            state.active = None;
            result.name = name;
            result.weight = 0.0;
        } else {
            let progress = (fade_elapsed / FADE_SECS) as f32;
            let weight = (target * (1.0 - progress)).max(0.0);
            result.name = name;
            result.weight = weight;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, target: f64, hold: f64, weight: f32) -> EmotionCommand {
        EmotionCommand {
            emotion: name.to_string(),
            target_time: target,
            hold_secs: hold,
            weight,
        }
    }

    #[test]
    fn empty_pipeline_returns_zero_weight() {
        let mut state = EmotionPipelineState::default();
        let applied = tick_emotions(&mut state, 0.0);
        assert_eq!(applied.weight, 0.0);
        assert!(state.active.is_none());
    }

    #[test]
    fn pop_command_becomes_active_at_target_time() {
        let mut state = EmotionPipelineState::default();
        state.pending.push_back(cmd("happy", 0.0, 2.0, 1.0));
        let applied = tick_emotions(&mut state, 0.0);
        assert_eq!(applied.name, "happy");
        assert_eq!(applied.weight, 1.0);
        assert!(state.active.is_some());
    }

    #[test]
    fn hold_window_keeps_weight() {
        let mut state = EmotionPipelineState::default();
        state.pending.push_back(cmd("happy", 0.0, 2.0, 1.0));
        tick_emotions(&mut state, 0.0); // consume
        let applied = tick_emotions(&mut state, 1.0); // 1s in
        assert_eq!(applied.name, "happy");
        assert_eq!(applied.weight, 1.0);
    }

    #[test]
    fn fade_after_hold_window() {
        let mut state = EmotionPipelineState::default();
        state.pending.push_back(cmd("happy", 0.0, 1.0, 1.0));
        tick_emotions(&mut state, 0.0); // hold_until = 1.0
        // Halfway through the 0.3s fade (t=1.15): weight ~ 0.5.
        let applied = tick_emotions(&mut state, 1.15);
        assert_eq!(applied.name, "happy");
        assert!(
            (applied.weight - 0.5).abs() < 0.05,
            "expected ~0.5 mid-fade, got {}",
            applied.weight
        );
    }

    #[test]
    fn fade_completes_and_clears_active() {
        let mut state = EmotionPipelineState::default();
        state.pending.push_back(cmd("happy", 0.0, 0.0, 1.0));
        tick_emotions(&mut state, 0.0); // hold_until = 0.0; immediately in fade
        let applied = tick_emotions(&mut state, 0.5); // 0.5s past hold_until > 0.3 fade
        assert_eq!(applied.weight, 0.0);
        assert!(state.active.is_none());
    }

    #[test]
    fn future_command_is_not_consumed() {
        let mut state = EmotionPipelineState::default();
        state.pending.push_back(cmd("happy", 10.0, 1.0, 1.0));
        let applied = tick_emotions(&mut state, 0.0);
        assert_eq!(applied.weight, 0.0);
        assert!(state.active.is_none());
        assert_eq!(state.pending.len(), 1);
    }
}
