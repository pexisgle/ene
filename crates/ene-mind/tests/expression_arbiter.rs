//! Expression arbiter integration tests (resolution, hysteresis, classifier hints).

#![expect(
    clippy::default_trait_access,
    reason = "tests use explicit Default::default for clarity"
)]

use std::time::Duration;

use ene_config::{CharacterCardV3, ExpressionAffect, ResolvedExpression, resolve_expressions};
use ene_mind::{
    EmotionConfig, ExpressionInput, ExpressionSource, OutputArbiter,
    output::{affect_to_expression, normalize_expression},
};
use ene_store::AffectState;

fn annotated(
    name: &str,
    valence: f32,
    arousal: f32,
    irritation: f32,
    fatigue: f32,
) -> ResolvedExpression {
    ResolvedExpression {
        name: name.into(),
        description: String::new(),
        vrm: Default::default(),
        affect: Some(ExpressionAffect {
            valence,
            arousal,
            irritation,
            fatigue,
        }),
    }
}

/// The production built-in defaults (via the real merge), so tests cannot
/// drift from what a default card resolves to at runtime.
fn default_expressions() -> Vec<ResolvedExpression> {
    resolve_expressions(&CharacterCardV3::default())
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
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "angry");
    assert_eq!(decision.source, ExpressionSource::AffectFallback);
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
        explicit_proposal: false,
        previous_expression: "sad",
        elapsed_since_change: Some(Duration::from_secs(1)),
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
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = engine.resolve(&config, &input);
    assert_eq!(decision.expression, "happy");
    assert_eq!(decision.source, ExpressionSource::Llm);
}

#[test]
fn llm_proposal_wins_when_affect_disagrees() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.irritation = 0.7;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("happy"),
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "happy");
    assert_eq!(decision.source, ExpressionSource::Llm);
}

#[test]
fn hysteresis_applies_to_llm_proposals_to_prevent_flicker() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.6;
    state.arousal = 0.3;
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("angry"),
        explicit_proposal: false,
        previous_expression: "sad",
        elapsed_since_change: Some(Duration::from_secs(1)),
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "sad");
    assert_eq!(decision.source, ExpressionSource::HysteresisHold);
}

#[test]
fn high_valence_maps_to_happy() {
    let mut state = AffectState::neutral("ene");
    state.valence = 0.5;
    state.arousal = 0.3;
    let available = default_expressions();
    assert_eq!(affect_to_expression(&state, &available), Some("happy"));
}

#[test]
fn japanese_card_does_not_misnormalize_expressions() {
    // A card that defines only Japanese expression names must not have an
    // English proposal fuzzy-matched onto one of them (the old character-level
    // levenshtein mapped 「怒り」 onto 「驚き」 with distance 2). The rejected
    // proposal falls back to the affect-mapped expression, keeping the face
    // consistent with the irritated state instead of the list's first name.
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.irritation = 0.7;
    let available = vec![
        annotated("にっこり", 0.6, 0.3, 0.0, 0.0),
        annotated("むすっ", -0.3, 0.2, 0.6, 0.0),
    ];

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("angry"),
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "むすっ");
    assert_eq!(decision.source, ExpressionSource::AffectFallback);
}

#[test]
fn neutral_state_maps_to_neutral_on_default_card() {
    // Regression guard: the production default card must show neutral at rest,
    // never the nearest emotional annotation (sad).
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let state = AffectState::neutral("ene");
    let available = default_expressions();

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: None,
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "neutral");
    assert_eq!(decision.source, ExpressionSource::AffectFallback);
}

#[test]
fn mixed_case_expression_names_resolve_case_insensitively() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.5;
    state.arousal = 0.3;
    let available = vec![
        annotated("Neutral", 0.0, 0.0, 0.0, 0.0),
        annotated("Happy", 0.6, 0.3, 0.0, 0.0),
    ];

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: Some("happy"),
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "Happy");
    assert_eq!(decision.source, ExpressionSource::Llm);
}

#[test]
fn japanese_card_resolves_from_affect_annotations() {
    let config = EmotionConfig::default();
    let arbiter = OutputArbiter;
    let mut state = AffectState::neutral("ene");
    state.valence = 0.5;
    state.arousal = 0.3;
    let available = vec![
        annotated("にっこり", 0.6, 0.3, 0.0, 0.0),
        annotated("むすっ", -0.3, 0.2, 0.6, 0.0),
    ];

    let input = ExpressionInput {
        affect: &state,
        available: &available,
        llm_proposal: None,
        explicit_proposal: false,
        previous_expression: "",
        elapsed_since_change: None,
        irritation_spike: false,
    };
    let decision = arbiter.resolve(&config, &input);
    assert_eq!(decision.expression, "にっこり");
    assert_eq!(decision.source, ExpressionSource::AffectFallback);
}
