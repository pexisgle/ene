use std::time::Duration;

use ene_card::AffectBaseline;
use ene_mind::{AffectProposal, EmotionConfig, EmotionEngine, TurnAffectInput};
use ene_store::AffectState;

#[test]
fn turn_without_classifier_leaves_affect_unchanged() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 2,
        baseline: AffectBaseline::default(),
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!((state.valence - 0.0).abs() < f32::EPSILON);
    assert!((state.affinity - 0.0).abs() < f32::EPSILON);
    assert!((state.irritation - 0.0).abs() < f32::EPSILON);
    assert!(
        !result
            .reasons
            .iter()
            .any(|r| matches!(r.category, "gratitude" | "insult" | "praise" | "urgency"))
    );
}

#[test]
fn decay_reduces_valence_over_time() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.8;

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::from_mins(30),
        recent_turn_count: 1,
        baseline: AffectBaseline::default(),
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
    };
    engine.update_turn(&config, &mut input);
    assert!(state.valence < 0.8);
}

#[test]
fn decay_converges_toward_card_baseline() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.8;

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::from_hours(24),
        recent_turn_count: 1,
        baseline: AffectBaseline {
            valence: 0.3,
            ..AffectBaseline::default()
        },
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
    };
    engine.update_turn(&config, &mut input);
    assert!((state.valence - 0.3).abs() < 0.01, "drifts to baseline");
}

#[test]
fn classifier_proposal_merged_when_confident() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let proposal = AffectProposal {
        user_emotion: "happy".into(),
        user_intent: "praise".into(),
        valence: 0.5,
        arousal: 0.2,
        irritation: 0.0,
        affinity: 0.4,
        recommended_expression: "happy".into(),
        confidence: 0.8,
        reason: "user praised".into(),
    };

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 1,
        baseline: AffectBaseline::default(),
        classifier_proposal: Some(proposal),
        classifier_min_confidence: 0.5,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!((state.valence - 0.4).abs() < 0.01);
    assert!(result.reasons.iter().any(|r| r.category == "classifier"));
}

#[test]
fn low_confidence_classifier_ignored() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let proposal = AffectProposal {
        user_emotion: "angry".into(),
        user_intent: "complaint".into(),
        valence: -0.8,
        arousal: 0.5,
        irritation: 0.9,
        affinity: -0.5,
        recommended_expression: "angry".into(),
        confidence: 0.2,
        reason: "uncertain".into(),
    };

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 1,
        baseline: AffectBaseline::default(),
        classifier_proposal: Some(proposal),
        classifier_min_confidence: 0.5,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!((state.valence - 0.0).abs() < f32::EPSILON);
    assert!(!result.reasons.iter().any(|r| r.category == "classifier"));
}

#[test]
fn fatigue_triggers_at_sixteen_user_turns_not_messages() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 16,
        baseline: AffectBaseline::default(),
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!(result.reasons.iter().any(|r| r.category == "fatigue"));
    assert!(state.fatigue > 0.0);

    let mut state2 = AffectState::neutral("ene");
    let mut input2 = TurnAffectInput {
        state: &mut state2,
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 15,
        baseline: AffectBaseline::default(),
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
    };
    let result2 = engine.update_turn(&config, &mut input2);
    assert!(!result2.reasons.iter().any(|r| r.category == "fatigue"));
}
