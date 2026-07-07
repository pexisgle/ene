//! Phase 7 emotion engine integration tests (#86, #88).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use ene_cognition::{AffectProposal, EmotionConfig, EmotionEngine, TurnAffectInput};
use ene_memory::AffectState;

#[test]
fn gratitude_raises_valence_deterministically() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "Thank you so much for your help!",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 2,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!(state.valence > 0.0);
    assert!(state.affinity > 0.0);
    assert!(result.reasons.iter().any(|r| r.category == "gratitude"));
}

#[test]
fn insult_lowers_valence_and_raises_irritation() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "I hate you, you're stupid",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 2,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    engine.update_turn(&config, &mut input);
    assert!(state.valence < 0.0);
    assert!(state.irritation > 0.0);
}

#[test]
fn decay_reduces_valence_over_time() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.8;

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "hello",
        elapsed_since_update: Duration::from_secs(30 * 60),
        recent_turn_count: 1,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    engine.update_turn(&config, &mut input);
    assert!(state.valence < 0.8);
}

#[test]
fn classifier_proposal_merged_when_confident() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let proposal = AffectProposal {
        user_emotion: "happy".into(),
        user_intent: "praise".into(),
        valence_delta: 0.2,
        arousal_delta: 0.1,
        irritation_delta: 0.0,
        affinity_delta: 0.1,
        recommended_expression: "happy".into(),
        confidence: 0.85,
        reason: "user praised".into(),
    };

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "nice",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 1,
        classifier_proposal: Some(proposal),
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!(state.valence > 0.0);
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
        valence_delta: -0.3,
        arousal_delta: 0.2,
        irritation_delta: 0.3,
        affinity_delta: -0.2,
        recommended_expression: "angry".into(),
        confidence: 0.2,
        reason: "uncertain".into(),
    };

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "ok",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 1,
        classifier_proposal: Some(proposal),
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!((state.valence - 0.0).abs() < f32::EPSILON);
    assert!(!result.reasons.iter().any(|r| r.category == "classifier"));
}

#[test]
fn llm_only_skips_deterministic_appraisal() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "Thank you so much!",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 2,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: true,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!(!result.reasons.iter().any(|r| r.category == "gratitude"));
    assert!((state.valence - 0.0).abs() < f32::EPSILON);
}

#[test]
fn fatigue_triggers_at_sixteen_user_turns_not_messages() {
    let config = EmotionConfig::default();
    let engine = EmotionEngine;
    let mut state = AffectState::neutral("ene");

    let mut input = TurnAffectInput {
        state: &mut state,
        user_message: "hello",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 16,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    let result = engine.update_turn(&config, &mut input);
    assert!(result.reasons.iter().any(|r| r.category == "fatigue"));
    assert!(state.fatigue > 0.0);

    let mut state2 = AffectState::neutral("ene");
    let mut input2 = TurnAffectInput {
        state: &mut state2,
        user_message: "hello",
        elapsed_since_update: Duration::ZERO,
        recent_turn_count: 15,
        classifier_proposal: None,
        classifier_min_confidence: 0.5,
        llm_only: false,
    };
    let result2 = engine.update_turn(&config, &mut input2);
    assert!(!result2.reasons.iter().any(|r| r.category == "fatigue"));
}
