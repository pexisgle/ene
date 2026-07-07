//! LLM-based affect classifier (#88).

use ene_config::PromptLibrary;
use ene_provider::{LlmMessage, LlmProvider, UserMessagePart};

use super::types::AffectProposal;
use crate::error::CognitionError;

/// Default timeout for the affect classifier LLM call in seconds.
pub const DEFAULT_CLASSIFIER_TIMEOUT_SECS: u64 = 15;

/// Classify user affect from a conversation snippet with a timeout budget.
pub async fn classify_with_timeout(
    provider: &dyn LlmProvider,
    conversation_snippet: &str,
    timeout_secs: u64,
    lang: &str,
) -> Result<AffectProposal, CognitionError> {
    let prompts = PromptLibrary::load(lang);

    let messages = vec![
        LlmMessage::System {
            content: prompts.affect_classifier().system.clone(),
        },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text {
                text: prompts
                    .affect_classifier()
                    .render_user_prompt(conversation_snippet),
            }],
        },
    ];

    let schema = classifier_schema();

    let raw = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        provider.chat_completion(&messages, Some(schema)),
    )
    .await
    .map_err(|_| {
        CognitionError::EmotionFailed(format!(
            "Affect classifier timed out after {timeout_secs}s"
        ))
    })?
    .map_err(|e| CognitionError::EmotionFailed(format!("LLM provider error: {e}")))?;

    parse_proposal_json(&raw)
}

fn classifier_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "user_emotion": { "type": "string" },
            "user_intent": { "type": "string" },
            "valence_delta": { "type": "number" },
            "arousal_delta": { "type": "number" },
            "irritation_delta": { "type": "number" },
            "affinity_delta": { "type": "number" },
            "recommended_expression": { "type": "string" },
            "confidence": { "type": "number" },
            "reason": { "type": "string" }
        },
        "required": [
            "user_emotion",
            "user_intent",
            "valence_delta",
            "arousal_delta",
            "irritation_delta",
            "affinity_delta",
            "recommended_expression",
            "confidence",
            "reason"
        ],
        "additionalProperties": false
    })
}

fn strip_markdown_fences(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(inner) = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        && let Some(inner) = inner.strip_suffix("```")
    {
        return inner.trim();
    }
    trimmed
}

fn parse_proposal_json(raw: &str) -> Result<AffectProposal, CognitionError> {
    let cleaned = strip_markdown_fences(raw);
    let value: serde_json::Value = serde_json::from_str(cleaned).map_err(|e| {
        CognitionError::EmotionFailed(format!("Failed to parse affect classifier JSON: {e}"))
    })?;

    let confidence = value
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as f32;

    Ok(AffectProposal {
        user_emotion: value
            .get("user_emotion")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        user_intent: value
            .get("user_intent")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        valence_delta: clamp_delta(
            value
                .get("valence_delta")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32,
            -0.3,
            0.3,
        ),
        arousal_delta: clamp_delta(
            value
                .get("arousal_delta")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32,
            -0.3,
            0.3,
        ),
        irritation_delta: clamp_delta(
            value
                .get("irritation_delta")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32,
            0.0,
            0.3,
        ),
        affinity_delta: clamp_delta(
            value
                .get("affinity_delta")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32,
            -0.3,
            0.3,
        ),
        recommended_expression: value
            .get("recommended_expression")
            .and_then(|v| v.as_str())
            .unwrap_or("neutral")
            .to_string(),
        confidence: if confidence.is_finite() {
            confidence.clamp(0.0, 1.0)
        } else {
            0.0
        },
        reason: value
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

fn clamp_delta(v: f32, min: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_proposal() {
        let raw = r#"{"user_emotion":"happy","user_intent":"praise","valence_delta":0.2,"arousal_delta":0.1,"irritation_delta":0.0,"affinity_delta":0.15,"recommended_expression":"happy","confidence":0.8,"reason":"user praised the assistant"}"#;
        let proposal = parse_proposal_json(raw).unwrap();
        assert_eq!(proposal.user_emotion, "happy");
        assert!((proposal.valence_delta - 0.2).abs() < f32::EPSILON);
        assert!((proposal.confidence - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn malformed_json_returns_error() {
        assert!(parse_proposal_json("not json").is_err());
    }

    #[test]
    fn strips_markdown_fences() {
        let raw = "```json\n{\"user_emotion\":\"neutral\",\"user_intent\":\"chat\",\"valence_delta\":0.0,\"arousal_delta\":0.0,\"irritation_delta\":0.0,\"affinity_delta\":0.0,\"recommended_expression\":\"neutral\",\"confidence\":0.3,\"reason\":\"small talk\"}\n```";
        let proposal = parse_proposal_json(raw).unwrap();
        assert_eq!(proposal.user_emotion, "neutral");
    }
}
