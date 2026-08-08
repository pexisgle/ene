//! Time-based affect decay toward per-character baselines.

use std::time::Duration;

use ene_card::AffectBaseline;
use ene_core::AffectState;

use super::types::{AffectDelta, AffectUpdateReason};

/// Apply exponential decay toward the character's affect baseline using
/// `half_life_minutes`. An all-zero baseline reproduces the legacy decay
/// toward zero exactly.
pub fn apply_decay(
    state: &mut AffectState,
    half_life_minutes: f64,
    elapsed: Duration,
    baseline: AffectBaseline,
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

    let baseline = baseline.clamp();
    let mut deltas = Vec::new();

    let decay_toward_baseline = |value: f32, baseline: f32| baseline + (value - baseline) * factor;

    let old_valence = state.valence;
    state.valence = decay_toward_baseline(state.valence, baseline.valence);
    if (state.valence - old_valence).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "valence",
            delta: state.valence - old_valence,
        });
    }

    let old_arousal = state.arousal;
    state.arousal = decay_toward_baseline(state.arousal, baseline.arousal);
    if (state.arousal - old_arousal).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "arousal",
            delta: state.arousal - old_arousal,
        });
    }

    let old_dominance = state.dominance;
    state.dominance = decay_toward_baseline(state.dominance, baseline.dominance);
    if (state.dominance - old_dominance).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "dominance",
            delta: state.dominance - old_dominance,
        });
    }

    let old_irritation = state.irritation;
    state.irritation = decay_toward_baseline(state.irritation, baseline.irritation);
    if (state.irritation - old_irritation).abs() > f32::EPSILON {
        deltas.push(AffectDelta {
            field: "irritation",
            delta: state.irritation - old_irritation,
        });
    }

    // Fatigue recovers slowly when idle (half the rate of mood decay).
    let fatigue_factor = 0.5_f64.powf(elapsed_minutes / (half_life_minutes * 2.0)) as f32;
    let old_fatigue = state.fatigue;
    state.fatigue = baseline.fatigue + (state.fatigue - baseline.fatigue) * fatigue_factor;
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
            "decayed toward baseline over {elapsed_minutes:.1} min (factor={factor:.3})"
        ),
        deltas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_core::AffectState;

    #[test]
    fn decay_moves_valence_toward_zero() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.8;
        let reason = apply_decay(
            &mut state,
            30.0,
            Duration::from_mins(30),
            AffectBaseline::default(),
        )
        .unwrap();
        assert!(state.valence < 0.8);
        assert!(state.valence > 0.0);
        assert_eq!(reason.category, "decay");
    }

    #[test]
    fn zero_elapsed_skips_decay() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        assert!(apply_decay(&mut state, 30.0, Duration::ZERO, AffectBaseline::default()).is_none());
        assert!((state.valence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn decay_converges_toward_baseline_not_zero() {
        let baseline = AffectBaseline {
            valence: 0.3,
            ..AffectBaseline::default()
        };
        let mut state = AffectState::neutral("ene");
        state.valence = 0.8;
        apply_decay(&mut state, 30.0, Duration::from_mins(30), baseline).unwrap();
        assert!(state.valence > 0.3);
        assert!(state.valence < 0.8);

        // Long enough for many half-lives: asymptotic to the baseline.
        let mut state2 = AffectState::neutral("ene");
        state2.valence = 0.8;
        apply_decay(&mut state2, 30.0, Duration::from_hours(24), baseline).unwrap();
        assert!((state2.valence - 0.3).abs() < 0.01);
    }

    #[test]
    fn all_zero_baseline_matches_legacy_decay() {
        let legacy = |value: f32| value * 0.5_f32;
        let mut state = AffectState::neutral("ene");
        state.valence = 0.8;
        state.irritation = 0.6;
        apply_decay(
            &mut state,
            30.0,
            Duration::from_mins(30),
            AffectBaseline::default(),
        )
        .unwrap();
        assert!((state.valence - legacy(0.8)).abs() < f32::EPSILON);
        assert!((state.irritation - legacy(0.6)).abs() < f32::EPSILON);
    }

    #[test]
    fn fatigue_decays_to_baseline_at_half_speed() {
        let baseline = AffectBaseline {
            fatigue: 0.4,
            ..AffectBaseline::default()
        };
        let mut state = AffectState::neutral("ene");
        state.fatigue = 1.0;
        let fatigue_factor = 0.5_f64.powf(30.0 / 60.0) as f32;
        apply_decay(&mut state, 30.0, Duration::from_mins(30), baseline).unwrap();
        let expected = 0.4 + (1.0 - 0.4) * fatigue_factor;
        assert!((state.fatigue - expected).abs() < f32::EPSILON);
    }
}
