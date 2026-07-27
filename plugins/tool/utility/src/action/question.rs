use std::fmt::Write;

use ene_plugin_proto::{MultiAnswer, QuestionItem, UserInputPrompt};
use ene_tool_sdk::prelude::*;
use uuid::Uuid;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "question",
    summary = "Ask the user a question (or multiple) and wait for a response.",
    description = "Presents one or more questions to the user through the UI. Each question can have predefined options, allow free-text input, or both. The LLM turn is paused until the user responds. Supports re-invocation with collected answers.",
    category = "Utility",
    keywords_primary = "question, ask, clarify, confirm, input"
)]
/// Action to ask the user one or more questions.
pub struct AskQuestionAction {
    /// One or more questions to present to the user. Each item renders
    /// its own input control in the UI; answers are returned in the
    /// same order.
    #[arg(min_items = 1)]
    questions: Vec<QuestionItem>,
    /// Set by the host on re-invocation with the user's responses, one
    /// per question in the same order as `questions`. Absent on the
    /// first call. The LLM must not populate this field.
    #[arg(internal)]
    #[serde(default)]
    user_answers: Option<Vec<MultiAnswer>>,
}

impl AskQuestionAction {
    async fn run(&self) -> Result<String, ToolError> {
        if let Some(answers) = &self.user_answers {
            return Ok(format_user_response(&self.questions, answers));
        }

        if self.questions.is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "No questions provided".to_string(),
            });
        }

        let prompt = UserInputPrompt::new(self.questions.clone())?;
        let request_id = Uuid::new_v4().to_string();
        Err(ToolError::UserInputRequired { request_id, prompt })
    }
}

fn format_user_response(questions: &[QuestionItem], answers: &[MultiAnswer]) -> String {
    let mut out = String::from("The user answered the questions:\n");
    for (i, q) in questions.iter().enumerate() {
        let rendered = answers
            .get(i)
            .map_or_else(|| "(no answer)".to_string(), format_answer);
        // `fmt::Error` is `Copy`, so `drop()` would itself trip
        // `clippy::dropping_copy_types`; writing into a `String` via
        // `fmt::Write` never actually fails.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "fmt::Write to a String is infallible in practice"
        )]
        let _ = writeln!(out, "{}. {} -> {}", i + 1, q.question, rendered);
    }
    out
}

fn format_answer(answer: &MultiAnswer) -> String {
    match answer {
        MultiAnswer::Selected { option } => format!("selected: {option}"),
        MultiAnswer::Answer { text } => format!("answered: {text}"),
        MultiAnswer::Skip => "(skipped)".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_deserialize_with_questions() {
        let json = r#"{"questions":[{"question":"Q?","options":["a","b"]}]}"#;
        let a: AskQuestionAction = serde_json::from_str(json).unwrap();
        assert_eq!(a.questions.len(), 1);
        assert_eq!(a.questions[0].options, vec!["a", "b"]);
        assert!(!a.questions[0].allow_free_text);
        assert!(a.user_answers.is_none());
    }

    #[test]
    fn args_reject_llm_hallucinated_fields() {
        let bad = r#"{"questions":[{"id":"mood","text":"How are you?","type":"free-text"}]}"#;
        let err = serde_json::from_str::<AskQuestionAction>(bad).unwrap_err();
        assert!(
            err.to_string().contains("question"),
            "expected missing-field error mentioning `question`, got: {err}"
        );
    }

    #[test]
    fn spec_uses_real_json_schema() {
        let spec = AskQuestionAction::spec();
        let item_schema = spec
            .parameters
            .get("properties")
            .and_then(|p| p.get("questions"))
            .and_then(|q| q.get("items"));
        let resolved = if let Some(items) = item_schema {
            if let Some(r) = items.get("$ref").and_then(|r| r.as_str()) {
                let def_name = r.rsplit('/').next().unwrap_or("");
                spec.parameters
                    .get("$defs")
                    .or_else(|| spec.parameters.get("definitions"))
                    .and_then(|d| d.get(def_name))
            } else {
                Some(items)
            }
        } else {
            None
        };
        let item_props = resolved.and_then(|i| i.get("properties"));
        assert!(
            item_props.and_then(|p| p.get("question")).is_some(),
            "schema must declare a `question` string field on each item"
        );
        assert!(
            item_props.and_then(|p| p.get("options")).is_some(),
            "schema must declare an `options` array field on each item"
        );
        assert!(
            item_props.and_then(|p| p.get("allow_free_text")).is_some(),
            "schema must declare an `allow_free_text` boolean field on each item"
        );
    }

    #[test]
    fn spec_does_not_expose_internal_user_answers() {
        let spec = AskQuestionAction::spec();
        let props = spec
            .parameters
            .get("properties")
            .and_then(|p| p.as_object());
        assert!(
            props.is_none_or(|p| !p.contains_key("user_answers")),
            "schema must not expose the host-injected `user_answers` field"
        );
    }

    #[test]
    fn spec_name_matches_action_name() {
        let action = AskQuestionAction::default();
        let spec = AskQuestionAction::spec();
        assert_eq!(action.name(), spec.name.as_str());
        assert_eq!(action.definition().name.as_str(), spec.name.as_str());
    }

    #[test]
    fn args_deserialize_with_user_answers() {
        let json = r#"{
            "questions":[{"question":"Q1"},{"question":"Q2"}],
            "user_answers":[
                {"kind":"answer","text":"hello"},
                {"kind":"skip"}
            ]
        }"#;
        let a: AskQuestionAction = serde_json::from_str(json).unwrap();
        let answers = a.user_answers.unwrap();
        assert_eq!(answers.len(), 2);
        assert!(matches!(&answers[0], MultiAnswer::Answer { text } if text == "hello"));
        assert!(matches!(&answers[1], MultiAnswer::Skip));
    }

    #[test]
    fn user_input_prompt_carries_items() {
        let qs = vec![
            QuestionItem {
                question: "Q1".into(),
                options: vec!["a".into(), "b".into()],
                allow_free_text: false,
            },
            QuestionItem {
                question: "Q2".into(),
                options: vec![],
                allow_free_text: true,
            },
        ];
        let p = UserInputPrompt::new(qs).unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].question, "Q1");
        assert_eq!(p.items[1].options, vec![] as Vec<String>);
    }

    #[test]
    fn format_user_response_single() {
        let qs = vec![QuestionItem {
            question: "Q?".into(),
            options: vec![],
            allow_free_text: true,
        }];
        let answers = vec![MultiAnswer::Answer { text: "yes".into() }];
        let s = format_user_response(&qs, &answers);
        assert!(s.contains("answered: yes"));
        assert!(s.contains("1. Q?"));
    }

    #[test]
    fn format_user_response_multiple_distinct() {
        let qs = vec![
            QuestionItem {
                question: "Q1".into(),
                options: vec!["a".into(), "b".into()],
                allow_free_text: false,
            },
            QuestionItem {
                question: "Q2".into(),
                options: vec![],
                allow_free_text: true,
            },
            QuestionItem {
                question: "Q3".into(),
                options: vec!["x".into(), "y".into()],
                allow_free_text: false,
            },
        ];
        let answers = vec![
            MultiAnswer::Selected { option: "a".into() },
            MultiAnswer::Answer {
                text: "alice".into(),
            },
            MultiAnswer::Skip,
        ];
        let s = format_user_response(&qs, &answers);
        assert!(s.contains("1. Q1 -> selected: a"));
        assert!(s.contains("2. Q2 -> answered: alice"));
        assert!(s.contains("3. Q3 -> (skipped)"));
    }

    #[test]
    fn format_user_response_missing_answers_marked() {
        let qs = vec![
            QuestionItem {
                question: "Q1".into(),
                options: vec![],
                allow_free_text: true,
            },
            QuestionItem {
                question: "Q2".into(),
                options: vec![],
                allow_free_text: true,
            },
        ];
        let answers = vec![MultiAnswer::Answer {
            text: "first".into(),
        }];
        let s = format_user_response(&qs, &answers);
        assert!(s.contains("1. Q1 -> answered: first"));
        assert!(s.contains("2. Q2 -> (no answer)"));
    }
}
