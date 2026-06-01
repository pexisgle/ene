use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use serde::Deserialize;

#[derive(Deserialize)]
struct QuestionArgs {
    questions: Vec<String>,
}

/// Action to ask the user clarifying questions.
pub struct AskQuestion;

#[async_trait]
impl ToolAction for AskQuestion {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "question".to_string(),
            description: concat!(
                "Asks the user clarifying questions when you need more information to proceed. ",
                "Use this tool when you are unsure about requirements, missing context, or need user confirmation."
            ).to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to ask the user",
                        "items": { "type": "string" }
                    }
                },
                "required": ["questions"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec!["question".to_string(), "ask".to_string(), "clarify".to_string(), "confirm".to_string()],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: QuestionArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for question: {e}"),
            })?;

        if args.questions.is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "No questions provided".to_string(),
            });
        }

        let formatted: Vec<String> = args
            .questions
            .iter()
            .enumerate()
            .map(|(i, q)| format!("{}. {}", i + 1, q))
            .collect();

        Ok(format!(
            "I need some clarification to proceed:\n{}\n\nPlease answer these questions and I'll continue.",
            formatted.join("\n")
        ))
    }
}
