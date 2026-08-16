//! Long-conversation fatigue (turn-count heuristic, language-agnostic).

use ene_core::AffectState;

use super::types::{AffectDelta, AffectUpdateReason};

const LONG_CONVERSATION_TURN_THRESHOLD: usize = 16;

const LONG_CONVERSATION_FATIGUE_DELTA: f32 = 0.03;

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
