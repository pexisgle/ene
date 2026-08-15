//! Emotion Engine: decay, conversation fatigue, optional LLM classifier merge.
//!
//! Same-turn reaction to the user utterance is left to the chat model. Affect
//! at prompt-build time is pre–current-utterance (decay + prior-turn classifier
//! + fatigue). Post-turn classification updates state for the next turn.

pub mod classifier;
mod decay;
mod fatigue;
pub mod types;

pub use types::{
    AffectDelta, AffectProposal, AffectUpdateReason, AffectUpdateResult, TurnAffectInput,
};

use ene_core::AffectState;

use crate::config::EmotionConfig;

use self::decay::apply_decay;
use self::fatigue::apply_conversation_fatigue;

pub struct EmotionEngine;

impl EmotionEngine {
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
            input.baseline,
        ) {
            reasons.push(reason);
        }

        if let Some(reason) = apply_conversation_fatigue(input.state, input.recent_turn_count) {
            reasons.push(reason);
        }

        if let Some(proposal) = &input.classifier_proposal
            && let Some(reason) =
                merge_classifier_proposal(input.state, proposal, input.classifier_min_confidence)
        {
            reasons.push(reason);
        }

        input.state.clamp();
        let mood_label = compute_mood_label(input.state);
        input.state.mood_label = mood_label.to_string();

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
            mood_label: mood_label.to_string(),
            reasons,
        }
    }
}

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

    apply_weighted_field(
        &mut state.valence,
        proposal.valence,
        weight,
        "valence",
        &mut deltas,
    );
    apply_weighted_field(
        &mut state.arousal,
        proposal.arousal,
        weight,
        "arousal",
        &mut deltas,
    );
    apply_weighted_field(
        &mut state.irritation,
        proposal.irritation,
        weight,
        "irritation",
        &mut deltas,
    );
    apply_weighted_field(
        &mut state.affinity,
        proposal.affinity,
        weight,
        "affinity",
        &mut deltas,
    );

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

fn apply_weighted_field(
    current: &mut f32,
    target: f32,
    weight: f32,
    field: &'static str,
    deltas: &mut Vec<AffectDelta>,
) {
    if !target.is_finite() || weight.abs() < f32::EPSILON {
        return;
    }
    let old = *current;
    *current = (target - *current).mul_add(weight, *current);
    let applied = *current - old;
    if applied.abs() >= f32::EPSILON {
        deltas.push(AffectDelta {
            field,
            delta: applied,
        });
    }
}

/// Human-readable mood label from PAD dimensions.
pub fn compute_mood_label(state: &AffectState) -> &'static str {
    if state.irritation >= 0.6 {
        return "irritated";
    }
    if state.fatigue >= 0.7 {
        return "tired";
    }
    if state.valence >= 0.4 && state.arousal >= 0.2 {
        return "cheerful";
    }
    if state.valence >= 0.3 {
        return "content";
    }
    if state.valence <= -0.4 && state.arousal >= 0.2 {
        return "upset";
    }
    if state.valence <= -0.3 {
        return "down";
    }
    if state.arousal >= 0.4 {
        return "alert";
    }
    if state.arousal <= -0.3 {
        return "calm";
    }
    if state.curiosity >= 0.5 {
        return "curious";
    }
    "neutral"
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "test asserts exact float equality")]
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
                elapsed_since_update: Duration::from_mins(1),
                recent_turn_count: 4,
                baseline: ene_card::AffectBaseline::default(),
                classifier_proposal: None,
                classifier_min_confidence: 0.5,
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
    fn values_clamp_after_extreme_classifier() {
        let config = EmotionConfig::default();
        let engine = EmotionEngine;
        let mut state = AffectState::neutral("ene");
        state.valence = 0.95;

        let proposal = AffectProposal {
            user_emotion: "angry".into(),
            user_intent: "insult".into(),
            valence: -2.0,
            arousal: 2.0,
            irritation: 2.0,
            affinity: -2.0,
            recommended_expression: "angry".into(),
            confidence: 1.0,
            reason: "extreme".into(),
        };

        let mut input = TurnAffectInput {
            state: &mut state,
            elapsed_since_update: Duration::ZERO,
            recent_turn_count: 2,
            baseline: ene_card::AffectBaseline::default(),
            classifier_proposal: Some(proposal),
            classifier_min_confidence: 0.5,
        };
        engine.update_turn(&config, &mut input);

        assert!(state.valence >= -1.0 && state.valence <= 1.0);
        assert!(state.irritation >= 0.0 && state.irritation <= 1.0);
    }
}
