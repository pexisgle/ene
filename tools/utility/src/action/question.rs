use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, MultiAnswer, QuestionItem, SideEffects, ToolCategory, ToolError, ToolExample,
    ToolName, ToolSpec, ToolVersion, UserInputPrompt,
};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize)]
struct QuestionArgs {
    /// One or more questions to present to the user.
    questions: Vec<QuestionItem>,
    /// Set by the host on re-invocation with the user's responses, one per
    /// question in the same order as `questions`. Absent on the first call.
    #[serde(default)]
    _user_answers: Option<Vec<MultiAnswer>>,
}

/// Action that asks the user clarifying questions and pauses the LLM turn
/// until the user provides answers through the UI.
pub struct AskQuestion;

#[async_trait]
impl ToolAction for AskQuestion {
    fn tool_name(&self) -> &'static str {
        "utility.question"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("utility.question"),
            version: ToolVersion::default(),
            display_name: "Ask the user a question (or multiple) and wait for a response.".to_string(),
            summary: "Ask the user a question (or multiple) and wait for a response.".to_string(),
            description: "Presents one or more questions to the user through the UI. Each question can have predefined options, allow free-text input, or both. The LLM turn is paused until the user responds. Supports re-invocation with collected answers.".to_string(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::primary_only(["question", "ask", "clarify", "confirm", "input"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One or more questions to present to the user. Each item renders its own input control in the UI; answers are returned in the same order.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The question text shown to the user."
                                },
                                "options": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "default": [],
                                    "description": "Predefined selectable options. Empty when the question accepts free text only."
                                },
                                "allow_free_text": {
                                    "type": "boolean",
                                    "default": false,
                                    "description": "Whether the user may type a custom answer that is not in `options`."
                                }
                            },
                            "required": ["question"],
                            "additionalProperties": false
                        }
                    },
                    "_user_answers": {
                        "type": "array",
                        "description": "Set by the host on re-invocation; do not populate on the first call.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {
                                    "type": "string",
                                    "enum": ["selected", "answer", "skip"]
                                },
                                "option": { "type": "string" },
                                "text": { "type": "string" }
                            },
                            "required": ["kind"]
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }),
            examples: vec![
                ToolExample {
                    description: "Ask a yes/no question with options".to_string(),
                    input: serde_json::json!({"questions": [{"question": "Do you want to proceed?", "options": ["yes", "no"]}]}),
                    output: Some("The user answered the questions:\n1. Do you want to proceed? -> selected: yes".to_string()),
                },
                ToolExample {
                    description: "Ask a free-text question".to_string(),
                    input: serde_json::json!({"questions": [{"question": "What is your name?", "options": [], "allow_free_text": true}]}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: QuestionArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for question: {e}"),
            })?;

        // Re-invocation path: the host already collected the user's per-question
        // answers and re-injected them under `_user_answers`. Format as a result.
        if let Some(answers) = args._user_answers {
            return Ok(format_user_response(&args.questions, &answers));
        }

        // First-call path: emit a single UserInputRequired prompt that lists
        // all questions; the UI will collect one answer per QuestionItem.
        if args.questions.is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "No questions provided".to_string(),
            });
        }

        let prompt = UserInputPrompt {
            items: args.questions.clone(),
        };
        let request_id = Uuid::new_v4().to_string();
        Err(ToolError::UserInputRequired { request_id, prompt })
    }
}

/// Formats the user's per-question answers as a human-readable string returned
/// to the LLM. Skipped questions are recorded as `(skipped)`; the LLM can
/// interpret that as "no answer provided".
fn format_user_response(questions: &[QuestionItem], answers: &[MultiAnswer]) -> String {
    let mut out = String::from("The user answered the questions:\n");
    for (i, q) in questions.iter().enumerate() {
        let rendered = answers
            .get(i)
            .map(format_answer)
            .unwrap_or_else(|| "(no answer)".to_string());
        out.push_str(&format!("{}. {} -> {}\n", i + 1, q.question, rendered));
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
        let a: QuestionArgs = serde_json::from_str(json).unwrap();
        assert_eq!(a.questions.len(), 1);
        assert_eq!(a.questions[0].options, vec!["a", "b"]);
        assert!(!a.questions[0].allow_free_text);
        assert!(a._user_answers.is_none());
    }

    #[test]
    fn args_reject_llm_hallucinated_fields() {
        // The LLM was emitting `{ "id", "text", "type" }` style items because
        // the spec's `parameters` was `{}`. With the real schema in place the
        // expected field is `question`, so this should fail deserialisation.
        let bad = r#"{"questions":[{"id":"mood","text":"How are you?","type":"free-text"}]}"#;
        let err = serde_json::from_str::<QuestionArgs>(bad).unwrap_err();
        assert!(
            err.to_string().contains("question"),
            "expected missing-field error mentioning `question`, got: {err}"
        );
    }

    #[test]
    fn spec_uses_real_json_schema() {
        let def = AskQuestion.definition();
        let props = def
            .parameters
            .get("properties")
            .and_then(|p| p.get("questions"));
        let item_props = props
            .and_then(|q| q.get("items"))
            .and_then(|i| i.get("properties"));
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
    fn args_deserialize_with_user_answers() {
        let json = r#"{
            "questions":[{"question":"Q1"},{"question":"Q2"}],
            "_user_answers":[
                {"kind":"answer","text":"hello"},
                {"kind":"skip"}
            ]
        }"#;
        let a: QuestionArgs = serde_json::from_str(json).unwrap();
        let answers = a._user_answers.unwrap();
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
        let p = UserInputPrompt { items: qs.clone() };
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
        // Only one answer for two questions; the second should be marked.
        let answers = vec![MultiAnswer::Answer {
            text: "first".into(),
        }];
        let s = format_user_response(&qs, &answers);
        assert!(s.contains("1. Q1 -> answered: first"));
        assert!(s.contains("2. Q2 -> (no answer)"));
    }
}
