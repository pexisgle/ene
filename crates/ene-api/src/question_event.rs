//! Live-bus question events shared by core and every client.

use serde::{Deserialize, Serialize};

/// Canonical live-bus event names for ask-user questions.
///
/// Core emits these through `QuestionEvent::to_value`; clients parse them
/// with `QuestionEvent::parse` instead of matching string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuestionEventKind {
    /// A job needs a user answer.
    Asked,
    /// Every open question on a job is closed by an answer or the timeout
    /// tick.
    Resolved,
}

impl QuestionEventKind {
    /// Wire name of the live-bus event.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Asked => "question.asked",
            Self::Resolved => "question.resolved",
        }
    }

    fn parse(event_type: &str) -> Option<Self> {
        match event_type {
            "question.asked" => Some(Self::Asked),
            "question.resolved" => Some(Self::Resolved),
            _ => None,
        }
    }
}

/// Typed view of one ask-user live-bus event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionEvent {
    /// Job (delegation) id that owns the questions; falls back to the soul id
    /// when the report carries no job.
    pub id: String,
    /// Soul the job belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soul_id: Option<String>,
    /// Combined ask-user speech shown to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Prompts of every still-open question on the job.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<String>,
    /// Wire-stable ids matching the questions; answers go back through the
    /// per-question answer route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub question_ids: Vec<String>,
}

impl QuestionEvent {
    /// Parse a raw live-bus JSON object into a typed question event.
    #[must_use]
    pub fn parse(value: &serde_json::Value) -> Option<(QuestionEventKind, Self)> {
        let kind = QuestionEventKind::parse(value.get("type")?.as_str()?);
        let id = value.get("id")?.as_str()?.to_owned();
        let prompt = value
            .get("prompt")
            .or_else(|| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let questions = string_array(value.get("questions"));
        let question_ids = string_array(value.get("question_ids"));
        let soul_id = value
            .get("soul_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        Some((
            kind?,
            Self {
                id,
                soul_id,
                prompt,
                questions,
                question_ids,
            },
        ))
    }

    /// Serialize into the canonical live-bus JSON shape.
    #[must_use]
    pub fn to_value(&self, kind: QuestionEventKind) -> serde_json::Value {
        let mut value = serde_json::json!({ "type": kind.as_str(), "id": self.id });
        let Some(obj) = value.as_object_mut() else {
            return value;
        };
        if let Some(soul_id) = &self.soul_id {
            obj.insert(
                "soul_id".to_owned(),
                serde_json::Value::String(soul_id.clone()),
            );
        }
        if let Some(prompt) = &self.prompt {
            obj.insert(
                "prompt".to_owned(),
                serde_json::Value::String(prompt.clone()),
            );
        }
        if !self.questions.is_empty() {
            obj.insert(
                "questions".to_owned(),
                serde_json::to_value(&self.questions).unwrap_or_default(),
            );
        }
        if !self.question_ids.is_empty() {
            obj.insert(
                "question_ids".to_owned(),
                serde_json::to_value(&self.question_ids).unwrap_or_default(),
            );
        }
        value
    }
}

fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{QuestionEvent, QuestionEventKind};

    #[test]
    fn round_trips_asked_event_with_ids() {
        let event = QuestionEvent {
            id: "job-1".to_owned(),
            soul_id: Some("soul-9".to_owned()),
            prompt: Some("which city?".to_owned()),
            questions: vec!["which city?".to_owned()],
            question_ids: vec!["q-1".to_owned()],
        };
        let raw = event.to_value(QuestionEventKind::Asked);
        assert_eq!(raw["type"], "question.asked");
        assert_eq!(raw["soul_id"], "soul-9");
        assert_eq!(raw["prompt"], "which city?");
        assert_eq!(raw["question_ids"][0], "q-1");
        let (kind, parsed) = QuestionEvent::parse(&raw).unwrap();
        assert_eq!(kind, QuestionEventKind::Asked);
        assert_eq!(parsed, event);
    }

    #[test]
    fn parses_resolved_event_without_optional_fields() {
        let (kind, parsed) = QuestionEvent::parse(&serde_json::json!({
            "type": "question.resolved",
            "id": "j"
        }))
        .unwrap();
        assert_eq!(kind, QuestionEventKind::Resolved);
        assert_eq!(parsed.id, "j");
        assert!(parsed.questions.is_empty());
    }
}
