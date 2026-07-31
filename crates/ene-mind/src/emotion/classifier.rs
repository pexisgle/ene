//! LLM-based affect classifier (#88).

use ene_ai::{LlmMessage, LlmProvider, UserMessagePart};
use ene_config::PromptLibrary;
use thiserror::Error;

use super::types::AffectProposal;
use crate::engine::ClassifierContext;
use crate::error::CognitionError;

/// Default `OpenRouter` model for post-turn affect classification.
pub const DEFAULT_CLASSIFIER_MODEL: &str = "google/gemini-2.5-flash-lite";

pub(crate) const DEFAULT_CLASSIFIER_TIMEOUT_SECS: u64 = 30;

const STREAM_FALLBACK_MAX_TOKENS: u32 = 512;

/// Errors from the affect classifier.
#[derive(Debug, Error)]
pub enum ClassifierError {
    /// Provider transport or API failure.
    #[error("LLM provider error: {0}")]
    Provider(#[from] ene_ai::LlmProviderError),

    /// JSON response could not be parsed.
    #[error("{0}")]
    Parse(String),

    /// Wall-clock budget exceeded.
    #[error("Affect classifier timed out after {0}s")]
    TimedOut(u64),

    /// Provider returned no usable text (common with some models when `max_tokens` is too low).
    #[error("Affect classifier received an empty response")]
    EmptyResponse,
}

/// How the classifier reaches the LLM backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClassifierTransport {
    NonStreaming,
    Streaming,
}

/// Classify using resilient transport fallback and a fresh provider per attempt.
pub async fn classify_for_config(
    config: &ene_config::EneConfig,
    model_override: Option<&str>,
    max_tokens: u32,
    context: &ClassifierContext,
    timeout_secs: u64,
    lang: &str,
) -> Result<AffectProposal, CognitionError> {
    let config = config.clone();
    let model_override = model_override
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned);
    let current_affect = context.current_affect.to_prompt_json();
    let conversation = context.conversation.clone();
    let lang = lang.to_owned();
    let configured_max_tokens = max_tokens;

    classify_with_resilient_fallback(
        move |attempt_cap| {
            let cap = attempt_cap.or(if configured_max_tokens > 0 {
                Some(configured_max_tokens)
            } else {
                None
            });
            let ai_cfg = config.get_section::<ene_ai::AiConfig>().map_err(|e| {
                ClassifierError::Provider(ene_ai::LlmProviderError::Provider(format!(
                    "Failed to parse AI config: {e}"
                )))
            })?;
            let mut resolved = ai_cfg
                .resolve_classifier()
                .map_err(ClassifierError::Provider)?;
            if let Some(override_model) = model_override.as_deref() {
                resolved.model = override_model.to_string();
            } else if resolved.model.is_empty() {
                resolved.model = DEFAULT_CLASSIFIER_MODEL.to_string();
            }
            if let Some(max) = cap {
                resolved.max_tokens = Some(max);
            }
            Ok(
                Box::new(ene_ai::create_chat_provider_from_resolved(&resolved))
                    as Box<dyn ene_ai::LlmProvider>,
            )
        },
        &current_affect,
        &conversation,
        timeout_secs,
        &lang,
    )
    .await
    .map_err(Into::into)
}

/// Map a classifier error to a short failure-reason label for logging.
pub const fn classify_failure_reason(error: &CognitionError) -> &'static str {
    match error {
        CognitionError::Classifier(classifier_error) => classifier_failure_reason(classifier_error),
        _ => "other",
    }
}

const fn classifier_failure_reason(error: &ClassifierError) -> &'static str {
    match error {
        ClassifierError::TimedOut(_) => "timeout",
        ClassifierError::EmptyResponse => "empty_response",
        ClassifierError::Parse(_) => "json_parse",
        ClassifierError::Provider(_) => "provider",
    }
}

/// JSON Schema for `response_format` with `strict: true` (OpenAI-compatible providers).
fn proposal_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "reason": { "type": "string" },
            "user_emotion": { "type": "string" },
            "user_intent": { "type": "string" },
            "valence": { "type": "number" },
            "arousal": { "type": "number" },
            "irritation": { "type": "number" },
            "affinity": { "type": "number" },
            "recommended_expression": { "type": "string" },
            "confidence": { "type": "number" }
        },
        "required": [
            "reason",
            "user_emotion",
            "user_intent",
            "valence",
            "arousal",
            "irritation",
            "affinity",
            "recommended_expression",
            "confidence"
        ],
        "additionalProperties": false
    })
}

async fn classify_with_resilient_fallback<F>(
    mut provider_factory: F,
    current_affect: &str,
    conversation: &str,
    timeout_secs: u64,
    lang: &str,
) -> Result<AffectProposal, ClassifierError>
where
    F: FnMut(Option<u32>) -> Result<Box<dyn LlmProvider>, ClassifierError>,
{
    let json_schema = proposal_json_schema();
    let attempts: [(ClassifierTransport, Option<u32>); 2] = [
        (ClassifierTransport::NonStreaming, None),
        (
            ClassifierTransport::Streaming,
            Some(STREAM_FALLBACK_MAX_TOKENS),
        ),
    ];

    let mut last_error = ClassifierError::TimedOut(timeout_secs);

    for (attempt_idx, (transport, max_tokens)) in attempts.into_iter().enumerate() {
        let provider = match provider_factory(max_tokens) {
            Ok(provider) => provider,
            Err(error) => return Err(error),
        };

        match classify_with_timeout(
            provider.as_ref(),
            current_affect,
            conversation,
            timeout_secs,
            lang,
            transport,
            &json_schema,
        )
        .await
        {
            Ok(proposal) => return Ok(proposal),
            Err(error) if attempt_idx == 0 => {
                last_error = error;
            }
            Err(error) => return Err(error),
        }
    }

    Err(last_error)
}

async fn classify_with_timeout(
    provider: &dyn LlmProvider,
    current_affect: &str,
    conversation: &str,
    timeout_secs: u64,
    lang: &str,
    transport: ClassifierTransport,
    json_schema: &serde_json::Value,
) -> Result<AffectProposal, ClassifierError> {
    let prompts = PromptLibrary::load(lang);
    let user_prompt = prompts
        .affect_classifier()
        .render_user_prompt(current_affect, conversation);

    let messages = vec![
        LlmMessage::System {
            content: prompts.affect_classifier().system.clone(),
        },
        LlmMessage::User {
            parts: vec![UserMessagePart::Text { text: user_prompt }],
        },
    ];

    let raw = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        call_provider(provider, &messages, transport, json_schema),
    )
    .await;

    let raw = match raw {
        Ok(Ok(raw)) => {
            if raw.trim().is_empty() {
                return Err(ClassifierError::EmptyResponse);
            }
            raw
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(ClassifierError::TimedOut(timeout_secs)),
    };

    parse_proposal_json(&raw)
}

async fn call_provider(
    provider: &dyn LlmProvider,
    messages: &[LlmMessage],
    transport: ClassifierTransport,
    json_schema: &serde_json::Value,
) -> Result<String, ClassifierError> {
    match transport {
        ClassifierTransport::NonStreaming => provider
            .chat_completion(messages, Some(json_schema.clone()))
            .await
            .map(|completion| completion.text)
            .map_err(ClassifierError::Provider),
        ClassifierTransport::Streaming => provider
            .collect_stream_completion(messages)
            .await
            .map(|completion| completion.text)
            .map_err(ClassifierError::Provider),
    }
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

fn parse_proposal_json(raw: &str) -> Result<AffectProposal, ClassifierError> {
    let cleaned = strip_markdown_fences(raw);

    let value: serde_json::Value = serde_json::from_str(cleaned).map_err(|e| {
        ClassifierError::Parse(format!("Failed to parse affect classifier JSON: {e}"))
    })?;

    let confidence = value
        .get("confidence")
        .and_then(serde_json::Value::as_f64)
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
        valence: clamp_absolute(
            value
                .get("valence")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0) as f32,
            -1.0,
            1.0,
        ),
        arousal: clamp_absolute(
            value
                .get("arousal")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0) as f32,
            -1.0,
            1.0,
        ),
        irritation: clamp_absolute(
            value
                .get("irritation")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0) as f32,
            0.0,
            1.0,
        ),
        affinity: clamp_absolute(
            value
                .get("affinity")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0) as f32,
            -1.0,
            1.0,
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

const fn clamp_absolute(v: f32, min: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CognitionError;

    #[test]
    fn parse_valid_proposal() {
        let raw = r#"{"user_emotion":"happy","user_intent":"praise","valence":0.5,"arousal":0.2,"irritation":0.0,"affinity":0.6,"recommended_expression":"happy","confidence":0.8,"reason":"user praised the assistant"}"#;
        let proposal = parse_proposal_json(raw).unwrap();
        assert_eq!(proposal.user_emotion, "happy");
        assert!((proposal.valence - 0.5).abs() < f32::EPSILON);
        assert!((proposal.confidence - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn malformed_json_returns_error() {
        assert!(parse_proposal_json("not json").is_err());
    }

    #[test]
    fn strips_markdown_fences() {
        let raw = "```json\n{\"user_emotion\":\"neutral\",\"user_intent\":\"chat\",\"valence\":0.0,\"arousal\":0.0,\"irritation\":0.0,\"affinity\":0.0,\"recommended_expression\":\"neutral\",\"confidence\":0.3,\"reason\":\"small talk\"}\n```";
        let proposal = parse_proposal_json(raw).unwrap();
        assert_eq!(proposal.user_emotion, "neutral");
    }

    #[test]
    fn classify_failure_reason_labels() {
        let timeout = CognitionError::Classifier(ClassifierError::TimedOut(15));
        assert_eq!(classify_failure_reason(&timeout), "timeout");

        let parse = CognitionError::Classifier(ClassifierError::Parse("bad".into()));
        assert_eq!(classify_failure_reason(&parse), "json_parse");

        let provider = CognitionError::Classifier(ClassifierError::Provider(
            ene_ai::LlmProviderError::Provider("boom".into()),
        ));
        assert_eq!(classify_failure_reason(&provider), "provider");

        let empty = CognitionError::Classifier(ClassifierError::EmptyResponse);
        assert_eq!(classify_failure_reason(&empty), "empty_response");
    }
}
