//! Emotion Engine: deterministic affect computation + optional LLM classifier (#86, #88).

mod appraisal;
pub mod classifier;
mod decay;
pub mod types;

pub use types::{
    AffectDelta, AffectProposal, AffectUpdateReason, AffectUpdateResult, TurnAffectInput,
};

use ene_store::AffectState;

use crate::config::EmotionConfig;

use self::appraisal::apply_appraisal;
use self::decay::apply_decay;

/// Emotion Engine: deterministic + optional LLM affect computation.
pub struct EmotionEngine;

impl EmotionEngine {
    /// Update affect state for the current turn: decay, appraisal, optional classifier merge.
    pub fn update_turn(
        &self,
        config: &EmotionConfig,
        input: &mut TurnAffectInput<'_>,
    ) -> AffectUpdateResult {
        let mut reasons = Vec::new();

        if let Some(reason) = apply_decay(
            input.state,
            config.decay_half_life_minutes,
            input.elapsed_since_update,
        ) {
            reasons.push(reason);
        }

        if !input.llm_only {
            reasons.extend(apply_appraisal(
                input.state,
                input.user_message,
                input.recent_turn_count,
            ));
        }

        if let Some(proposal) = &input.classifier_proposal
            && let Some(reason) =
                merge_classifier_proposal(input.state, proposal, input.classifier_min_confidence)
        {
            reasons.push(reason);
        }

        input.state.clamp();
        let mood_label = compute_mood_label(input.state);
        input.state.mood_label.clone_from(&mood_label);

        tracing::debug!(
            component = "EmotionEngine",
            mood = %mood_label,
            valence = input.state.valence,
            arousal = input.state.arousal,
            irritation = input.state.irritation,
            fatigue = input.state.fatigue,
            reason_count = reasons.len(),
            "Affect state updated"
        );

        for reason in &reasons {
            tracing::debug!(
                component = "EmotionEngine",
                category = reason.category,
                detail = %reason.detail,
                "Affect update reason"
            );
        }

        AffectUpdateResult {
            mood_label,
            reasons,
        }
    }
}

/// Merge an advisory LLM classifier absolute estimate with confidence-weighted blending.
fn merge_classifier_proposal(
    state: &mut AffectState,
    proposal: &AffectProposal,
    min_confidence: f32,
) -> Option<AffectUpdateReason> {
    if !proposal.confidence.is_finite() || proposal.confidence < min_confidence {
        return None;
    }

    let weight = proposal.confidence.clamp(0.0, 1.0);
    let mut deltas = Vec::new();

    apply_weighted_blend(state, "valence", proposal.valence, weight, &mut deltas);
    apply_weighted_blend(state, "arousal", proposal.arousal, weight, &mut deltas);
    apply_weighted_blend(
        state,
        "irritation",
        proposal.irritation,
        weight,
        &mut deltas,
    );
    apply_weighted_blend(state, "affinity", proposal.affinity, weight, &mut deltas);

    if deltas.is_empty() {
        return None;
    }

    Some(AffectUpdateReason {
        category: "classifier",
        detail: if proposal.reason.is_empty() {
            format!(
                "LLM absolute estimate (confidence={weight:.2}, user_emotion={})",
                proposal.user_emotion
            )
        } else {
            format!(
                "LLM absolute estimate (confidence={weight:.2}): {}",
                proposal.reason
            )
        },
        deltas,
    })
}

fn apply_weighted_blend(
    state: &mut AffectState,
    field: &'static str,
    target: f32,
    weight: f32,
    deltas: &mut Vec<AffectDelta>,
) {
    if !target.is_finite() || weight.abs() < f32::EPSILON {
        return;
    }
    match field {
        "valence" => {
            let old = state.valence;
            state.valence = (target - state.valence).mul_add(weight, state.valence);
            let applied = state.valence - old;
            if applied.abs() >= f32::EPSILON {
                deltas.push(AffectDelta {
                    field,
                    delta: applied,
                });
            }
        }
        "arousal" => {
            let old = state.arousal;
            state.arousal = (target - state.arousal).mul_add(weight, state.arousal);
            let applied = state.arousal - old;
            if applied.abs() >= f32::EPSILON {
                deltas.push(AffectDelta {
                    field,
                    delta: applied,
                });
            }
        }
        "irritation" => {
            let old = state.irritation;
            state.irritation = (target - state.irritation).mul_add(weight, state.irritation);
            let applied = state.irritation - old;
            if applied.abs() >= f32::EPSILON {
                deltas.push(AffectDelta {
                    field,
                    delta: applied,
                });
            }
        }
        "affinity" => {
            let old = state.affinity;
            state.affinity = (target - state.affinity).mul_add(weight, state.affinity);
            let applied = state.affinity - old;
            if applied.abs() >= f32::EPSILON {
                deltas.push(AffectDelta {
                    field,
                    delta: applied,
                });
            }
        }
        _ => {}
    }
}

/// Derive a human-readable mood label from PAD dimensions.
#[must_use]
pub fn compute_mood_label(state: &AffectState) -> String {
    if state.irritation >= 0.6 {
        return "irritated".into();
    }
    if state.fatigue >= 0.7 {
        return "tired".into();
    }
    if state.valence >= 0.4 && state.arousal >= 0.2 {
        return "cheerful".into();
    }
    if state.valence >= 0.3 {
        return "content".into();
    }
    if state.valence <= -0.4 && state.arousal >= 0.2 {
        return "upset".into();
    }
    if state.valence <= -0.3 {
        return "down".into();
    }
    if state.arousal >= 0.4 {
        return "alert".into();
    }
    if state.arousal <= -0.3 {
        return "calm".into();
    }
    if state.curiosity >= 0.5 {
        return "curious".into();
    }
    "neutral".into()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::config::EmotionConfig;

    #[test]
    fn update_turn_is_deterministic() {
        let config = EmotionConfig::default();
        let engine = EmotionEngine;

        let run = || {
            let mut state = AffectState::neutral("ene");
            state.valence = 0.2;
            let mut input = TurnAffectInput {
                state: &mut state,
                user_message: "Thank you!",
                elapsed_since_update: Duration::from_secs(60),
                recent_turn_count: 4,
                classifier_proposal: None,
                classifier_min_confidence: 0.5,
                llm_only: false,
            };
            engine.update_turn(&config, &mut input);
            state
        };

        let a = run();
        let b = run();
        assert_eq!(a.valence, b.valence);
        assert_eq!(a.affinity, b.affinity);
        assert_eq!(a.mood_label, b.mood_label);
    }

    #[test]
    fn values_clamp_after_extreme_input() {
        let config = EmotionConfig::default();
        let engine = EmotionEngine;
        let mut state = AffectState::neutral("ene");
        state.valence = 0.95;

        let insult = "I hate you, you're stupid and useless!!!";
        let mut input = TurnAffectInput {
            state: &mut state,
            user_message: insult,
            elapsed_since_update: Duration::ZERO,
            recent_turn_count: 2,
            classifier_proposal: None,
            classifier_min_confidence: 0.5,
            llm_only: false,
        };
        engine.update_turn(&config, &mut input);

        assert!(state.valence >= -1.0 && state.valence <= 1.0);
        assert!(state.irritation >= 0.0 && state.irritation <= 1.0);
    }
}
