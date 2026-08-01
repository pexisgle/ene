//! Decision JSON schema and fail-closed parser.

#![expect(
    clippy::string_slice,
    reason = "special-token and summarizer parsers slice at known ASCII delimiters"
)]
use crate::proactive::{ProactiveDecision, ProactiveUrgency};
use serde_json::{Value, json};

/// Plain JSON Schema for proactive decision structured output.
#[must_use]
pub fn decision_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["screen_digest", "reason", "should_speak", "confidence", "topic_hint", "urgency"],
        "properties": {
            "screen_digest": { "type": "string" },
            "reason": { "type": "string" },
            "should_speak": { "type": "boolean" },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "topic_hint": { "type": "string" },
            "urgency": { "type": "string", "enum": ["low", "normal", "high"] }
        }
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
    let screen_digest = obj
        .get("screen_digest")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let reason = obj
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let mut topic_hint = obj
        .get("topic_hint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    // topic_hint must not copy reason or screen_digest verbatim.
    if !topic_hint.is_empty() && (topic_hint == reason || topic_hint == screen_digest) {
        topic_hint.clear();
    }
    let urgency = ProactiveUrgency::parse(obj.get("urgency").and_then(Value::as_str));

    ProactiveDecision {
        should_speak,
        confidence,
        screen_digest,
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
    fn decision_schema_is_json_schema_root() {
        let schema = decision_schema();
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
        assert!(schema.get("schema").is_none());
        assert!(schema.get("name").is_none());
        assert!(schema.get("strict").is_none());
    }

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
    fn parses_screen_digest_before_reason_field_order() {
        let d = parse_decision_json(
            r#"{"screen_digest":"Text editor.\nEditing source.","reason":"idle with open thread","should_speak":true,"confidence":0.7,"topic_hint":"check in","urgency":"normal"}"#,
        );
        assert!(d.should_speak);
        assert_eq!(d.screen_digest, "Text editor.\nEditing source.");
        assert_eq!(d.reason, "idle with open thread");
        assert_eq!(d.topic_hint, "check in");
    }

    #[test]
    fn missing_screen_digest_defaults_empty() {
        let d = parse_decision_json(
            r#"{"reason":"idle","should_speak":false,"confidence":0.5,"topic_hint":"","urgency":"low"}"#,
        );
        assert!(d.screen_digest.is_empty());
        assert!(!d.should_speak);
    }

    #[test]
    fn clears_topic_hint_that_copies_screen_digest_verbatim() {
        let d = parse_decision_json(
            r#"{"screen_digest":"same text","reason":"other","should_speak":true,"confidence":0.9,"topic_hint":"same text","urgency":"normal"}"#,
        );
        assert!(d.should_speak);
        assert!(d.topic_hint.is_empty());
    }

    #[test]
    fn extracts_json_from_fenced_noise() {
        let d = parse_decision_json(
            "Sure.\n```json\n{\"should_speak\":false,\"confidence\":0.1,\"reason\":\"x\",\"topic_hint\":\"\",\"urgency\":\"normal\"}\n```",
        );
        assert!(!d.should_speak);
        assert!((d.confidence - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn clears_topic_hint_that_copies_reason_verbatim() {
        let d = parse_decision_json(
            r#"{"reason":"same text","should_speak":true,"confidence":0.9,"topic_hint":"same text","urgency":"normal"}"#,
        );
        assert!(d.should_speak);
        assert_eq!(d.reason, "same text");
        assert!(d.topic_hint.is_empty());
    }
}
