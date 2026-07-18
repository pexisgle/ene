//! Decision JSON schema and fail-closed parser.

use crate::proactive::{ProactiveDecision, ProactiveUrgency};
use serde_json::{Value, json};

/// Inner JSON Schema object (grammar / strict validation).
#[must_use]
pub fn decision_schema_object() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["should_speak", "confidence", "reason", "topic_hint", "urgency"],
        "properties": {
            "should_speak": { "type": "boolean" },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "reason": { "type": "string" },
            "topic_hint": { "type": "string" },
            "urgency": { "type": "string", "enum": ["low", "normal", "high"] }
        }
    })
}

/// JSON Schema passed to cloud providers that support structured output wrappers.
#[must_use]
pub fn decision_schema() -> Value {
    json!({
        "name": "proactive_decision",
        "strict": true,
        "schema": decision_schema_object(),
    })
}

/// Parse model output into a normalized [`ProactiveDecision`].
///
/// Unknown fields are ignored. Invalid / missing values fail closed toward silence.
#[must_use]
pub fn parse_decision_json(raw: &str) -> ProactiveDecision {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return ProactiveDecision::silent("empty decision response");
    }

    let text = extract_json_object(trimmed).unwrap_or(trimmed);
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return ProactiveDecision::silent("decision json parse failed");
    };
    let Some(obj) = value.as_object() else {
        return ProactiveDecision::silent("decision json was not an object");
    };

    let should_speak = obj
        .get("should_speak")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let confidence = match obj.get("confidence").and_then(Value::as_f64) {
        Some(c) if c.is_finite() && (0.0..=1.0).contains(&c) => c,
        Some(_) => {
            return ProactiveDecision::silent("confidence out of range");
        }
        None => 0.0,
    };
    let reason = obj
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let topic_hint = obj
        .get("topic_hint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let urgency = ProactiveUrgency::parse(obj.get("urgency").and_then(Value::as_str));

    ProactiveDecision {
        should_speak,
        confidence,
        reason,
        topic_hint,
        urgency,
    }
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&raw[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_decision() {
        let d = parse_decision_json(
            r#"{"should_speak":true,"confidence":0.8,"reason":"idle","topic_hint":"hi","urgency":"high","extra":1}"#,
        );
        assert!(d.should_speak);
        assert!((d.confidence - 0.8).abs() < f64::EPSILON);
        assert_eq!(d.urgency, ProactiveUrgency::High);
        assert_eq!(d.topic_hint, "hi");
    }

    #[test]
    fn empty_and_invalid_fail_closed() {
        assert!(!parse_decision_json("").should_speak);
        assert!(!parse_decision_json("not json").should_speak);
        assert!(!parse_decision_json(r#"{"should_speak":"yes"}"#).should_speak);
    }

    #[test]
    fn confidence_out_of_range_fails_closed() {
        let d = parse_decision_json(r#"{"should_speak":true,"confidence":2.5}"#);
        assert!(!d.should_speak);
        assert!(d.reason.contains("confidence"));
    }

    #[test]
    fn extracts_json_from_fenced_noise() {
        let d = parse_decision_json(
            "Sure.\n```json\n{\"should_speak\":false,\"confidence\":0.1,\"reason\":\"x\",\"topic_hint\":\"\",\"urgency\":\"normal\"}\n```",
        );
        assert!(!d.should_speak);
        assert!((d.confidence - 0.1).abs() < f64::EPSILON);
    }
}
