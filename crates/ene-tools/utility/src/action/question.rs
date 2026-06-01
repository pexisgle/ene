use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, UserInputPrompt};
use ene_tools_common::ToolAction;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

/// A single question that can be answered by selecting an option or by typing
/// a free-text response.
#[derive(Debug, Clone, Deserialize)]
struct QuestionItem {
    /// The question text shown to the user.
    question: String,
    /// Predefined selectable options. Empty when only free-text is allowed.
    #[serde(default)]
    options: Vec<String>,
    /// Whether the user can supply a custom answer that is not in `options`.
    #[serde(default)]
    allow_free_text: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct QuestionArgs {
    /// One or more questions to present to the user.
    questions: Vec<QuestionItem>,
    /// Set by the host on re-invocation with the user's response.
    /// Absent on the first call.
    #[serde(default)]
    _user_response: Option<String>,
}

/// Action that asks the user clarifying questions and pauses the LLM turn
/// until the user provides an answer through the UI.
pub struct AskQuestion;

#[async_trait]
impl ToolAction for AskQuestion {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "question".to_string(),
            description: concat!(
                "Asks the user clarifying questions when you need more information to proceed. ",
                "Each question may declare a list of `options` (rendered as selectable buttons) ",
                "and `allow_free_text` (true to enable a custom-text input). ",
                "The tool call blocks until the user answers; the response is then returned to you."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to present to the user. Multiple questions are shown together in one dialog.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The question text."
                                },
                                "options": {
                                    "type": "array",
                                    "description": "Predefined selectable options. Omit or leave empty for free-text only.",
                                    "items": { "type": "string" }
                                },
                                "allow_free_text": {
                                    "type": "boolean",
                                    "description": "Whether the user can type a custom answer. Defaults to false.",
                                    "default": false
                                }
                            },
                            "required": ["question"]
                        },
                        "minItems": 1
                    }
                },
                "required": ["questions"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec![
                "question".to_string(),
                "ask".to_string(),
                "clarify".to_string(),
                "confirm".to_string(),
                "input".to_string(),
            ],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: QuestionArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for question: {e}"),
            })?;

        // Re-invocation path: the host already collected the user's response
        // and re-injected it under `_user_response`. Format it as a result.
        if let Some(response) = args._user_response {
            return Ok(format_user_response(&args.questions, &response));
        }

        // First-call path: build a single prompt from all questions and emit
        // UserInputRequired so the UI pauses the LLM turn.
        if args.questions.is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "No questions provided".to_string(),
            });
        }

        let prompt = build_prompt(&args.questions);
        let request_id = Uuid::new_v4().to_string();
        Err(ToolError::UserInputRequired { request_id, prompt })
    }
}

/// Combines all sub-questions into a single [`UserInputPrompt`]. The header
/// reflects whether this is a yes/no style or a multiple-choice question.
fn build_prompt(questions: &[QuestionItem]) -> UserInputPrompt {
    let question_text = questions
        .iter()
        .enumerate()
        .map(|(i, q)| format!("{}. {}", i + 1, q.question))
        .collect::<Vec<_>>()
        .join("\n");

    let mut all_options: Vec<String> = Vec::new();
    for q in questions {
        all_options.extend(q.options.iter().cloned());
    }
    all_options.sort();
    all_options.dedup();

    let allow_free_text = questions.iter().any(|q| q.allow_free_text);

    let header = if !all_options.is_empty() {
        Some("MultipleChoice".to_string())
    } else {
        Some("FreeText".to_string())
    };

    UserInputPrompt {
        header,
        question: question_text,
        options: all_options,
        allow_free_text,
    }
}

/// Formats the user's response as a human-readable string returned to the LLM.
fn format_user_response(questions: &[QuestionItem], response: &str) -> String {
    if questions.len() == 1 {
        return format!("The user answered: {}", response);
    }
    let mut out = String::from("The user answered the questions:\n");
    for (i, q) in questions.iter().enumerate() {
        out.push_str(&format!("{}. {} -> {}\n", i + 1, q.question, response));
    }
    out
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
        assert!(a._user_response.is_none());
    }

    #[test]
    fn args_deserialize_with_user_response() {
        let json = r#"{"questions":[{"question":"Q?"}],"_user_response":"my answer"}"#;
        let a: QuestionArgs = serde_json::from_str(json).unwrap();
        assert_eq!(a._user_response.as_deref(), Some("my answer"));
    }

    #[test]
    fn build_prompt_collects_options_dedup() {
        let qs = vec![
            QuestionItem {
                question: "Q1".into(),
                options: vec!["a".into(), "b".into()],
                allow_free_text: false,
            },
            QuestionItem {
                question: "Q2".into(),
                options: vec!["b".into(), "c".into()],
                allow_free_text: true,
            },
        ];
        let p = build_prompt(&qs);
        assert_eq!(p.options, vec!["a", "b", "c"]);
        assert!(p.allow_free_text);
        assert_eq!(p.header.as_deref(), Some("MultipleChoice"));
        assert!(p.question.contains("1. Q1"));
        assert!(p.question.contains("2. Q2"));
    }

    #[test]
    fn build_prompt_free_text_only() {
        let qs = vec![QuestionItem {
            question: "Type something".into(),
            options: vec![],
            allow_free_text: true,
        }];
        let p = build_prompt(&qs);
        assert!(p.options.is_empty());
        assert!(p.allow_free_text);
        assert_eq!(p.header.as_deref(), Some("FreeText"));
    }

    #[test]
    fn format_user_response_single() {
        let qs = vec![QuestionItem {
            question: "Q?".into(),
            options: vec![],
            allow_free_text: true,
        }];
        let s = format_user_response(&qs, "yes");
        assert!(s.contains("yes"));
    }

    #[test]
    fn format_user_response_multiple() {
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
        let s = format_user_response(&qs, "x");
        assert!(s.contains("1. Q1"));
        assert!(s.contains("2. Q2"));
        assert!(s.contains("x"));
    }
}
