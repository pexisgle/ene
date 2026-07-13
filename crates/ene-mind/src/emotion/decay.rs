//! Time-based affect decay toward neutral baselines (#86).

use std::time::Duration;

use ene_store::AffectState;

use super::types::{AffectDelta, AffectUpdateReason};

/// Apply exponential decay toward neutral baselines using `half_life_minutes`.
pub fn apply_decay(
    state: &mut AffectState,
    half_life_minutes: f64,
    elapsed: Duration,
) -> Option<AffectUpdateReason> {
    if half_life_minutes <= 0.0 || elapsed.is_zero() {
        return None;
    }

    let elapsed_minutes = elapsed.as_secs_f64() / 60.0;
    if elapsed_minutes <= 0.0 {
        return None;
    }

    let factor = 0.5_f64.powf(elapsed_minutes / half_life_minutes) as f32;
    if factor >= 1.0_f32 {
        return None;
    }

    let mut deltas = Vec::new();

    let decay_toward_zero = |value: f32| value * factor;

    let old_valence = state.valence;
    state.valence = decay_toward_zero(state.valence);
    if (state.valence - old_valence).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "valence",
            delta: state.valence - old_valence,
        });
    }

    let old_arousal = state.arousal;
    state.arousal = decay_toward_zero(state.arousal);
    if (state.arousal - old_arousal).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "arousal",
            delta: state.arousal - old_arousal,
        });
    }

    let old_dominance = state.dominance;
    state.dominance = decay_toward_zero(state.dominance);
    if (state.dominance - old_dominance).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "dominance",
            delta: state.dominance - old_dominance,
        });
    }

    let old_irritation = state.irritation;
    state.irritation *= factor;
    if (state.irritation - old_irritation).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "irritation",
            delta: state.irritation - old_irritation,
        });
    }

    // Fatigue recovers slowly when idle (half the rate of mood decay).
    let fatigue_factor = 0.5_f64.powf(elapsed_minutes / (half_life_minutes * 2.0)) as f32;
    let old_fatigue = state.fatigue;
    state.fatigue *= fatigue_factor;
    if (state.fatigue - old_fatigue).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "fatigue",
            delta: state.fatigue - old_fatigue,
        });
    }

    if deltas.is_empty() {
        return None;
    }

    Some(AffectUpdateReason {
        category: "decay",
        detail: format!(
            "decayed toward neutral over {:.1} min (factor={factor:.3})",
            elapsed_minutes
        ),
        deltas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_store::AffectState;

    #[test]
    fn decay_moves_valence_toward_zero() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.8;
        let reason = apply_decay(&mut state, 30.0, Duration::from_secs(30 * 60)).unwrap();
        assert!(state.valence < 0.8);
        assert!(state.valence > 0.0);
        assert_eq!(reason.category, "decay");
    }

    #[test]
    fn zero_elapsed_skips_decay() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        assert!(apply_decay(&mut state, 30.0, Duration::ZERO).is_none());
        assert!((state.valence - 0.5).abs() < f32::EPSILON);
    }
}
