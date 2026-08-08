//! Expression resolution: affect mapping, LLM hints, hysteresis.

use ene_card::{ExpressionAffect, ResolvedExpression};
use ene_core::AffectState;

use super::types::{ExpressionDecision, ExpressionInput, ExpressionSource};
use crate::config::EmotionConfig;

/// Affect states within this per-dimension distance of the origin count as
/// neutral (resting) states — the old hardcoded thresholds only left neutral
/// for `|valence| < 0.35`, `arousal < 0.45`, `irritation < 0.55`, and
/// `fatigue < 0.65`, so the guard band sits well inside that region.
const NEUTRAL_STATE_RADIUS: f32 = 0.1;

/// An annotation must lie within this per-dimension distance of the origin to
/// be accepted as the resting expression of a neutral state. The built-in
/// emotional annotations all lie farther out (sad is the closest at 0.5 on
/// valence), so an unannotated-origin card cannot map a neutral state onto a
/// sad face; the caller falls back to neutral instead.
const REST_POINT_RADIUS: f32 = 0.2;

/// Map affect dimensions to the nearest annotated expression.
///
/// Each expression carries its own affect point (card-side annotation), so no
/// expression name is hardcoded here: cards define any name — including
/// Japanese or `x_`-prefixed custom ones — and the runtime picks the nearest
/// annotated expression in affect space. Returns `None` when no expression
/// carries an annotation, or when the state is neutral and the card has no
/// annotation near the origin; the caller falls back to neutral.
///
/// Ties between equally distant annotations resolve to the first in
/// `available` order — `resolve_expressions` puts built-in defaults before
/// card overrides, so defaults win ties.
pub fn affect_to_expression<'a>(
    state: &AffectState,
    available: &'a [ResolvedExpression],
) -> Option<&'a str> {
    let neutral_state = state.valence.abs() <= NEUTRAL_STATE_RADIUS
        && state.arousal.abs() <= NEUTRAL_STATE_RADIUS
        && state.irritation.abs() <= NEUTRAL_STATE_RADIUS
        && state.fatigue.abs() <= NEUTRAL_STATE_RADIUS;
    available
        .iter()
        .filter_map(|e| e.affect.map(|a| (e, a)))
        .filter(|(_, a)| {
            !neutral_state
                || (a.valence.abs() <= REST_POINT_RADIUS
                    && a.arousal.abs() <= REST_POINT_RADIUS
                    && a.irritation.abs() <= REST_POINT_RADIUS
                    && a.fatigue.abs() <= REST_POINT_RADIUS)
        })
        .min_by(|(_, a), (_, b)| affect_distance(a, state).total_cmp(&affect_distance(b, state)))
        .map(|(e, _)| e.name.as_str())
}

/// Squared Euclidean distance between an affect annotation and a state over
/// all four dimensions.
///
/// An omitted annotation dimension is 0.0 (neutral on that axis), so the
/// state's value on that axis counts fully into the distance.
fn affect_distance(annotation: &ExpressionAffect, state: &AffectState) -> f32 {
    let d_valence = annotation.valence - state.valence;
    let d_arousal = annotation.arousal - state.arousal;
    let d_irritation = annotation.irritation - state.irritation;
    let d_fatigue = annotation.fatigue - state.fatigue;
    d_valence * d_valence
        + d_arousal * d_arousal
        + d_irritation * d_irritation
        + d_fatigue * d_fatigue
}

/// Resolve the final expression from affect, LLM hints, and constraints.
///
/// An LLM proposal is canonical when present; an out-of-list proposal is
/// rejected and falls back to the affect-mapped expression (or neutral when
/// no annotation data exists), never to fuzzy string matching. Affect mapping
/// is the fallback when the model emitted no expression marker. Hysteresis
/// applies to every source so rapid mid-turn markers cannot flicker the face,
/// except that an explicit streamed `[expr:...]` marker bypasses the hold —
/// the model's explicit instruction wins. Speech-timed expression changes are
/// owned by a separate timing path.
pub fn resolve_expression(
    config: &EmotionConfig,
    input: &ExpressionInput<'_>,
) -> ExpressionDecision {
    let available_names: Vec<String> = input
        .available
        .iter()
        .map(|e| e.name.to_lowercase())
        .collect();

    let affect_candidate =
        affect_to_expression(input.affect, input.available).map(str::to_lowercase);
    let mut candidate = affect_candidate
        .clone()
        .unwrap_or_else(|| fallback_name(&available_names));
    let mut source = ExpressionSource::AffectFallback;
    let mut reason = format!("mapped from affect (mood={})", input.affect.mood_label);
    if affect_candidate.is_none() {
        source = ExpressionSource::FallbackNeutral;
        reason = "no affect-annotated expression; using neutral".into();
    }

    if config.llm_can_propose_expression
        && let Some(proposal) = input.llm_proposal
        && !proposal.trim().is_empty()
    {
        let proposal_lower = proposal.trim().to_lowercase();
        if available_names.iter().any(|n| n == &proposal_lower) {
            candidate = proposal_lower;
            source = ExpressionSource::Llm;
            reason = format!("LLM expression proposal `{proposal}`");
        } else if let Some(affect_name) = &affect_candidate {
            candidate.clone_from(affect_name);
            source = ExpressionSource::AffectFallback;
            reason = format!(
                "unsupported LLM expression proposal `{proposal}`; using affect-mapped expression `{affect_name}`"
            );
        } else {
            candidate = fallback_name(&available_names);
            source = ExpressionSource::FallbackNeutral;
            reason = format!(
                "unsupported LLM expression proposal `{proposal}`; no affect-mapped expression; using neutral"
            );
        }
    }

    // Hysteresis gates every source except explicit streamed markers:
    // without it, each streamed LLM marker would snap the face mid-turn.
    // An explicit `[expr:...]` marker is the model's direct instruction and
    // bypasses the hold; classifier hints and affect sources stay gated.
    // Speech-aligned timing is a separate concern.
    if !input.irritation_spike
        && !input.explicit_proposal
        && !input.previous_expression.is_empty()
        && candidate != input.previous_expression.to_lowercase()
        && let Some(elapsed) = input.elapsed_since_change
        && elapsed.as_secs_f64() < config.expression_hysteresis_seconds
    {
        let held = normalize_expression(input.previous_expression, &available_names);
        return ExpressionDecision {
            expression: canonical_name(&held, input.available)
                .map(str::to_string)
                .unwrap_or(held),
            reason: format!(
                "hysteresis hold ({:.1}s < {:.1}s)",
                elapsed.as_secs_f64(),
                config.expression_hysteresis_seconds
            ),
            source: ExpressionSource::HysteresisHold,
        };
    }

    if !available_names.iter().any(|n| n == &candidate) {
        candidate = normalize_expression("neutral", &available_names);
        source = ExpressionSource::FallbackNeutral;
        reason = "unsupported expression; fell back to neutral".into();
    }

    ExpressionDecision {
        expression: canonical_name(&candidate, input.available)
            .map(str::to_string)
            .unwrap_or(candidate),
        reason,
        source,
    }
}

/// Map a lowercased name back to the card's original-cased spelling, so the
/// emitted cue matches the name the card (and its VRM keys) actually uses.
fn canonical_name<'a>(lower: &str, available: &'a [ResolvedExpression]) -> Option<&'a str> {
    available
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(lower))
        .map(|e| e.name.as_str())
}

/// Normalize an expression name against available expressions.
///
/// Only an exact (case-insensitive) match is accepted; anything else falls
/// back to neutral (or the first available expression). Fuzzy matching is
/// deliberately absent: character-level similarity mis-maps short Japanese
/// names (「怒り」 vs 「驚き」 are two edits apart).
pub fn normalize_expression(name: &str, available: &[String]) -> String {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() {
        return fallback_name(available);
    }
    if available.iter().any(|n| n == &lower) {
        return lower;
    }
    fallback_name(available)
}

/// Last-resort expression when no affect data supports a choice.
///
/// "neutral" is the conventional resting name and is preferred when the card
/// defines it; otherwise the first listed expression is used, and a literal
/// "neutral" only when the list is empty (the cue then names an expression
/// the card does not have, leaving the VRM at its resting blendshape).
fn fallback_name(available: &[String]) -> String {
    if available.iter().any(|n| n == "neutral") {
        "neutral".into()
    } else {
        available
            .first()
            .cloned()
            .unwrap_or_else(|| "neutral".into())
    }
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_card::{CharacterCardV3, ExpressionAffect, ResolvedExpression, resolve_expressions};
    use ene_core::AffectState;

    /// The production built-in defaults (via the real merge), so tests cannot
    /// drift from what a default card resolves to at runtime.
    fn default_available() -> Vec<ResolvedExpression> {
        resolve_expressions(&CharacterCardV3::default())
    }

    fn names(exprs: &[ResolvedExpression]) -> Vec<String> {
        exprs.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn high_valence_maps_to_happy() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available = default_available();
        assert_eq!(affect_to_expression(&state, &available), Some("happy"));
    }

    #[test]
    fn unsupported_llm_falls_back_to_neutral() {
        let available = default_available();
        let names_list: Vec<String> = names(&available);
        assert_eq!(
            normalize_expression("nonexistent_emotion", &names_list),
            "neutral"
        );
    }

    #[test]
    fn japanese_proposal_does_not_fuzzy_match_dissimilar_expression() {
        // 「怒り」 and 「驚き」 are two edit operations apart under the old
        // character-level levenshtein; exact matching must reject it.
        let available = default_available();
        let names_list: Vec<String> = names(&available);
        assert_eq!(normalize_expression("怒り", &names_list), "neutral");
        assert_eq!(normalize_expression("喜び", &names_list), "neutral");
    }

    #[test]
    fn aliases_are_no_longer_expanded() {
        let available = default_available();
        let names_list: Vec<String> = names(&available);
        assert_eq!(normalize_expression("joy", &names_list), "neutral");
        assert_eq!(normalize_expression("mad", &names_list), "neutral");
    }

    #[test]
    fn hysteresis_holds_previous_expression() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.valence = 0.6;
        state.arousal = 0.3;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "sad");
        assert_eq!(decision.source, ExpressionSource::HysteresisHold);
    }

    #[test]
    fn hysteresis_holds_even_when_llm_proposes_change() {
        // Rapid LLM markers would flicker without source-agnostic hysteresis.
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.valence = 0.6;
        state.arousal = 0.3;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("angry"),
            explicit_proposal: false,
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "sad");
        assert_eq!(decision.source, ExpressionSource::HysteresisHold);
    }

    #[test]
    fn explicit_llm_marker_bypasses_hysteresis() {
        // An explicit streamed marker is the model's direct instruction and
        // wins even when the previous expression would otherwise be held.
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.valence = 0.6;
        state.arousal = 0.3;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("angry"),
            explicit_proposal: true,
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::Llm);
    }

    #[test]
    fn llm_proposal_wins_over_disagreeing_affect() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("happy"),
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "happy");
        assert_eq!(decision.source, ExpressionSource::Llm);
    }

    #[test]
    fn resolves_without_llm_token() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::AffectFallback);
    }

    #[test]
    fn unannotated_expressions_fall_back_to_neutral() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available: Vec<ResolvedExpression> = ["neutral", "smile", "frown"]
            .into_iter()
            .map(|name| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
                affect: None,
            })
            .collect();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "neutral");
        assert_eq!(decision.source, ExpressionSource::FallbackNeutral);
    }

    fn japanese_available() -> Vec<ResolvedExpression> {
        [
            (
                "neutral",
                ExpressionAffect {
                    valence: 0.0,
                    arousal: 0.0,
                    irritation: 0.0,
                    fatigue: 0.0,
                },
            ),
            (
                "にっこり",
                ExpressionAffect {
                    valence: 0.6,
                    arousal: 0.3,
                    irritation: 0.0,
                    fatigue: 0.0,
                },
            ),
            (
                "むすっ",
                ExpressionAffect {
                    valence: -0.3,
                    arousal: 0.2,
                    irritation: 0.6,
                    fatigue: 0.0,
                },
            ),
        ]
        .into_iter()
        .map(|(name, affect)| ResolvedExpression {
            name: name.into(),
            description: String::new(),
            vrm: Default::default(),
            affect: Some(affect),
        })
        .collect()
    }

    #[test]
    fn japanese_annotated_expressions_resolve_from_affect() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available = japanese_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "にっこり");
        assert_eq!(decision.source, ExpressionSource::AffectFallback);
    }

    #[test]
    fn out_of_list_llm_proposal_falls_back_to_affect_candidate() {
        // An out-of-list proposal must not be fuzzy-matched onto a Japanese
        // expression (the old levenshtein would map 「怒り」 onto 「驚き」).
        // Instead of the neutral-named fallback, the affect-mapped expression
        // keeps the face consistent with the state (irritated → むすっ).
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available = japanese_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("happy"),
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "むすっ");
        assert_eq!(decision.source, ExpressionSource::AffectFallback);
    }

    #[test]
    fn out_of_list_llm_proposal_without_annotations_falls_back_to_neutral() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available: Vec<ResolvedExpression> = ["neutral", "smile", "frown"]
            .into_iter()
            .map(|name| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
                affect: None,
            })
            .collect();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("angry"),
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "neutral");
        assert_eq!(decision.source, ExpressionSource::FallbackNeutral);
    }

    #[test]
    fn neutral_state_maps_to_neutral_on_default_card() {
        // Regression: an all-zero (resting) state must never resolve to a
        // distant emotional annotation such as sad.
        let state = AffectState::neutral("ene");
        let available = default_available();
        assert_eq!(affect_to_expression(&state, &available), Some("neutral"));
        let config = EmotionConfig::default();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "neutral");
        assert_eq!(decision.source, ExpressionSource::AffectFallback);
    }

    #[test]
    fn slightly_nonzero_state_stays_neutral() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.08;
        state.arousal = -0.06;
        let available = default_available();
        assert_eq!(affect_to_expression(&state, &available), Some("neutral"));
    }

    #[test]
    fn neutral_state_without_rest_annotation_has_no_confident_match() {
        // A card whose only annotations are emotional has no resting face:
        // a neutral state must not be forced onto the nearest emotion.
        let state = AffectState::neutral("ene");
        let available: Vec<ResolvedExpression> = [
            (
                "にっこり",
                ExpressionAffect {
                    valence: 0.6,
                    arousal: 0.3,
                    irritation: 0.0,
                    fatigue: 0.0,
                },
            ),
            (
                "むすっ",
                ExpressionAffect {
                    valence: -0.3,
                    arousal: 0.2,
                    irritation: 0.6,
                    fatigue: 0.0,
                },
            ),
        ]
        .into_iter()
        .map(|(name, affect)| ResolvedExpression {
            name: name.into(),
            description: String::new(),
            vrm: Default::default(),
            affect: Some(affect),
        })
        .collect();
        assert_eq!(affect_to_expression(&state, &available), None);
    }

    #[test]
    fn mixed_case_expression_names_keep_card_spelling() {
        // Cards may define capitalized names; the affect path and the LLM
        // proposal path must both match case-insensitively and emit the
        // card's original spelling.
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        state.arousal = 0.3;
        let available: Vec<ResolvedExpression> = [
            ("Neutral", [0.0, 0.0, 0.0, 0.0]),
            ("Happy", [0.6, 0.3, 0.0, 0.0]),
            ("Angry", [-0.2, 0.3, 0.7, 0.0]),
        ]
        .into_iter()
        .map(
            |(name, [valence, arousal, irritation, fatigue])| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
                affect: Some(ExpressionAffect {
                    valence,
                    arousal,
                    irritation,
                    fatigue,
                }),
            },
        )
        .collect();

        assert_eq!(affect_to_expression(&state, &available), Some("Happy"));

        let config = EmotionConfig::default();
        let affect_input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: None,
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &affect_input);
        assert_eq!(decision.expression, "Happy");
        assert_eq!(decision.source, ExpressionSource::AffectFallback);

        let mut angry_state = AffectState::neutral("ene");
        angry_state.irritation = 0.7;
        let llm_input = ExpressionInput {
            affect: &angry_state,
            available: &available,
            llm_proposal: Some("angry"),
            explicit_proposal: false,
            previous_expression: "",
            elapsed_since_change: None,
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &llm_input);
        assert_eq!(decision.expression, "Angry");
        assert_eq!(decision.source, ExpressionSource::Llm);
    }

    #[test]
    fn hysteresis_hold_keeps_card_spelling() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.valence = 0.6;
        state.arousal = 0.3;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("happy"),
            explicit_proposal: false,
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "sad");
        assert_eq!(decision.source, ExpressionSource::HysteresisHold);
    }

    #[test]
    fn high_valence_with_irritation_maps_to_happy() {
        // Documented divergence from the legacy priority chain (which returned
        // angry for irritation >= 0.55 before checking valence): nearest
        // neighbour weighs valence too.
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        state.arousal = 0.3;
        state.irritation = 0.55;
        let available = default_available();
        assert_eq!(affect_to_expression(&state, &available), Some("happy"));
    }

    #[test]
    fn fatigued_negative_valence_maps_to_sad() {
        // Divergence from the legacy priority chain (fatigue >= 0.65 won over
        // everything): the annotation for relaxed sits at valence 0.2, so a
        // strongly negative state maps to sad instead.
        let mut state = AffectState::neutral("ene");
        state.valence = -0.5;
        state.fatigue = 0.7;
        let available = default_available();
        assert_eq!(affect_to_expression(&state, &available), Some("sad"));
    }
}
