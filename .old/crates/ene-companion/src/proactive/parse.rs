use crate::proactive::{ProactiveDecision, ProactiveUrgency};
use serde_json::{Value, json};

#[must_use]
pub fn decision_schema_object() -> Value {
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
        Some(_) => return ProactiveDecision::silent("confidence out of range"),
        None => 0.0,
    };
    let screen_digest = obj
        .get("screen_digest")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let reason = obj
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut topic_hint = obj
        .get("topic_hint")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
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
    (end >= start).then(|| &raw[start..=end])
}
