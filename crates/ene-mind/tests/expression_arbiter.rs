//! Expression arbiter integration tests (resolution, hysteresis, classifier hints).

#![expect(
    clippy::default_trait_access,
    reason = "tests use explicit Default::default for clarity"
)]

use std::time::Duration;

use ene_config::ResolvedExpression;
use ene_mind::{
    EmotionConfig, ExpressionInput, ExpressionSource, OutputArbiter,
    output::{affect_to_expression, normalize_expression},
};
use ene_store::AffectState;

fn default_expressions() -> Vec<ResolvedExpression> {
    ["neutral", "happy", "sad", "angry", "relaxed", "surprised"]
        .into_iter()
        .map(|name| ResolvedExpression {
            name: name.into(),
            description: String::new(),
            vrm: Default::default(),
        })
        .collect()
}

#[test]
fn resolves_expression_without_llm_token() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.irritation = 0.7;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: None,
        previous_expression: "",
        elapsed_since_change: None,
        response_text: "I understand.",
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "angry");
    assert_eq!(decision.source, ExpressionSource::AffectMapping);
}

#[test]
fn hysteresis_prevents_rapid_expression_change() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.6;
    state.arousal = 0.3;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: None,
        previous_expression: "sad",
        elapsed_since_change: Some(Duration::from_secs(1)),
        response_text: "",
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "sad");
    assert_eq!(decision.source, ExpressionSource::HysteresisHold);
}

#[test]
fn unsupported_expression_falls_back_to_neutral() {
    let names: Vec<String> = default_expressions()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert_eq!(
        normalize_expression("unknown_emotion_xyz", &names),
        "neutral"
    );
}

#[test]
fn classifier_hint_used_when_no_stream_token() {
    let config = EmotionConfig::default();
    let engine = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.1;
    state.arousal = 0.0;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("happy"),
        previous_expression: "",
        elapsed_since_change: None,
        response_text: "Wonderful!",
        irritation_spike: false,
    };
    let decision = engine.resolve(&config, &input);
    assert_eq!(decision.expression, "happy");
    assert_eq!(decision.source, ExpressionSource::LlmAdvisory);
}

#[test]
fn advisory_llm_does_not_override_strong_affect() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.irritation = 0.7;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("happy"),
        previous_expression: "",
        elapsed_since_change: None,
        response_text: "",
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "angry");
    assert_eq!(decision.source, ExpressionSource::AffectMapping);
}

#[test]
fn high_valence_maps_to_happy() {
    let mut state = AffectState::neutral("ene");
    state.valence = 0.5;
    state.arousal = 0.3;
    assert_eq!(affect_to_expression(&state), "happy");
}
