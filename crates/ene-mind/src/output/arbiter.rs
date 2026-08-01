//! Expression resolution: affect mapping, LLM hints, hysteresis.

#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "mind pipeline uses intentional turn/score/index arithmetic; history/token helpers index into bounds-checked conversational buffers"
)]
use ene_core::AffectState;

use super::types::{ExpressionDecision, ExpressionInput, ExpressionSource};
use crate::config::EmotionConfig;

/// Map affect dimensions to a candidate expression name.
pub fn affect_to_expression(state: &AffectState) -> &'static str {
    if state.irritation >= 0.55 {
        return "angry";
    }
    if state.fatigue >= 0.65 {
        return "relaxed";
    }
    if state.valence >= 0.35 && state.arousal >= 0.15 {
        return "happy";
    }
    if state.valence <= -0.35 {
        return "sad";
    }
    if state.arousal >= 0.45 {
        return "surprised";
    }
    "neutral"
}

/// Resolve the final expression from affect, LLM hints, and constraints.
pub fn resolve_expression(
    config: &EmotionConfig,
    input: &ExpressionInput<'_>,
) -> ExpressionDecision {
    let available_names: Vec<String> = input
        .available
        .iter()
        .map(|e| e.name.to_lowercase())
        .collect();

    let affect_candidate = affect_to_expression(input.affect).to_string();
    let mut candidate = normalize_expression(&affect_candidate, &available_names);
    let mut source = ExpressionSource::AffectMapping;
    let mut reason = format!("mapped from affect (mood={})", input.affect.mood_label);

    if config.llm_can_propose_expression
        && let Some(proposal) = input.llm_proposal
        && !proposal.trim().is_empty()
    {
        let normalized = normalize_expression(&proposal.to_lowercase(), &available_names);
        if config.llm_expression_is_advisory {
            // Adopt LLM hint only when affect is neutral or agrees with the proposal.
            if candidate == "neutral" && normalized != "neutral" {
                candidate = normalized;
                source = ExpressionSource::LlmAdvisory;
                reason = format!("LLM advisory proposal `{proposal}` (affect neutral)");
            } else if normalized == candidate {
                source = ExpressionSource::LlmAdvisory;
                reason = format!("LLM advisory agrees with affect mapping `{candidate}`");
            } else {
                tracing::debug!(
                    component = "OutputArbiter",
                    affect_candidate = %candidate,
                    llm_proposal = %normalized,
                    "Ignoring LLM advisory; affect mapping takes precedence"
                );
            }
        } else {
            candidate = normalized;
            source = ExpressionSource::LlmCommand;
            reason = format!("LLM expression command `{proposal}`");
        }
    }

    // Lightweight response-text sentiment nudge.
    if matches!(source, ExpressionSource::AffectMapping)
        && input.response_text.contains('!')
        && input.affect.valence > 0.0
    {
        let excited = normalize_expression("happy", &available_names);
        if excited != candidate {
            candidate = excited;
            reason = "response exclamation + positive valence".into();
        }
    }

    // Hysteresis: hold previous expression unless irritation spike
    // or the current candidate comes from an authoritative source
    // (LLM command/advisory). Hysteresis only gates affect-driven
    // and fallback choices.
    if !input.irritation_spike
        && !input.previous_expression.is_empty()
        && candidate != input.previous_expression
        && let Some(elapsed) = input.elapsed_since_change
        && elapsed.as_secs_f64() < config.expression_hysteresis_seconds
        && matches!(
            source,
            ExpressionSource::AffectMapping | ExpressionSource::FallbackNeutral
        )
    {
        let held = normalize_expression(input.previous_expression, &available_names);
        return ExpressionDecision {
            expression: held,
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
        expression: candidate,
        reason,
        source,
    }
}

/// Normalize an expression name against available expressions; nearest match or neutral.
pub fn normalize_expression(name: &str, available: &[String]) -> String {
    let lower = name.trim().to_lowercase();
    if lower.is_empty() {
        return fallback_name(available);
    }
    if available.iter().any(|n| n == &lower) {
        return lower;
    }
    let alias = match lower.as_str() {
        "joy" | "joyful" | "excited" => "happy",
        "anger" | "mad" | "upset" => "angry",
        "calm" | "tired" | "sleepy" => "relaxed",
        "shock" | "shocked" => "surprised",
        "sorrow" | "unhappy" => "sad",
        other => other,
    };
    if available.iter().any(|n| n == alias) {
        return alias.to_string();
    }
    if let Some(best) = available.iter().min_by_key(|n| levenshtein(&lower, n))
        && levenshtein(&lower, best) <= 2
    {
        return best.clone();
    }
    fallback_name(available)
}

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

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut curr = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr.push((prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost));
        }
        prev = curr;
    }
    prev[b.len()]
}

#[cfg(test)]
#[expect(
    clippy::default_trait_access,
    reason = "explicit Default for test fixture clarity"
)]
mod tests {
    use super::*;
    use ene_config::ResolvedExpression;
    use ene_core::AffectState;

    fn default_available() -> Vec<ResolvedExpression> {
        ["neutral", "happy", "sad", "angry", "relaxed", "surprised"]
            .into_iter()
            .map(|name| ResolvedExpression {
                name: name.into(),
                description: String::new(),
                vrm: Default::default(),
            })
            .collect()
    }

    fn names(exprs: &[ResolvedExpression]) -> Vec<String> {
        exprs.iter().map(|e| e.name.clone()).collect()
    }

    #[test]
    fn high_valence_maps_to_happy() {
        let mut state = AffectState::neutral("ene");
        state.valence = 0.5;
        state.arousal = 0.3;
        assert_eq!(affect_to_expression(&state), "happy");
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
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            response_text: "",
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "sad");
        assert_eq!(decision.source, ExpressionSource::HysteresisHold);
    }

    #[test]
    fn llm_command_bypasses_hysteresis() {
        let config = EmotionConfig {
            llm_expression_is_advisory: false,
            ..Default::default()
        };
        let mut state = AffectState::neutral("ene");
        state.valence = 0.6;
        state.arousal = 0.3;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("angry"),
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            response_text: "",
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::LlmCommand);
    }

    #[test]
    fn llm_advisory_bypasses_hysteresis() {
        let config = EmotionConfig {
            llm_expression_is_advisory: true,
            ..Default::default()
        };
        let state = AffectState::neutral("ene");
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("angry"),
            previous_expression: "sad",
            elapsed_since_change: Some(std::time::Duration::from_secs(1)),
            response_text: "",
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::LlmAdvisory);
    }

    #[test]
    fn advisory_llm_does_not_override_strong_affect() {
        let config = EmotionConfig::default();
        let mut state = AffectState::neutral("ene");
        state.irritation = 0.7;
        let available = default_available();
        let input = ExpressionInput {
            affect: &state,
            available: &available,
            llm_proposal: Some("happy"),
            previous_expression: "",
            elapsed_since_change: None,
            response_text: "",
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::AffectMapping);
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
            previous_expression: "",
            elapsed_since_change: None,
            response_text: "I understand.",
            irritation_spike: false,
        };
        let decision = resolve_expression(&config, &input);
        assert_eq!(decision.expression, "angry");
        assert_eq!(decision.source, ExpressionSource::AffectMapping);
    }
}
