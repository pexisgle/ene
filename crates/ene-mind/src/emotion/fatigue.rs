//! Long-conversation fatigue (turn-count heuristic, language-agnostic).

#![expect(
    clippy::arithmetic_side_effects,
    reason = "fatigue delta is a fixed bounded increment clamped to [0, 1]"
)]

use ene_core::AffectState;

use super::types::{AffectDelta, AffectUpdateReason};

/// Threshold of recent user turns before long-conversation fatigue applies.
const LONG_CONVERSATION_TURN_THRESHOLD: usize = 16;

/// Fatigue increment per turn beyond the long-conversation threshold.
const LONG_CONVERSATION_FATIGUE_DELTA: f32 = 0.03;

/// Apply a small fatigue bump when the conversation has many recent user turns.
pub fn apply_conversation_fatigue(
    state: &mut AffectState,
    recent_turn_count: usize,
) -> Option<AffectUpdateReason> {
    if recent_turn_count < LONG_CONVERSATION_TURN_THRESHOLD {
        return None;
    }

    let old = state.fatigue;
    state.fatigue = (state.fatigue + LONG_CONVERSATION_FATIGUE_DELTA).clamp(0.0, 1.0);
    let delta = state.fatigue - old;
    if delta.abs() < f32::EPSILON {
        return None;
    }

    Some(AffectUpdateReason {
        category: "fatigue",
        detail: format!(
            "long conversation ({recent_turn_count} recent turns >= {LONG_CONVERSATION_TURN_THRESHOLD})"
        ),
        deltas: vec![AffectDelta {
            field: "fatigue",
            delta,
        }],
    })
}
