use super::definition::ToolDefinition;
use crate::error::AiCoreError;

pub fn tool_definition() -> ToolDefinition {
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
    }
}

pub fn question(questions: Vec<String>) -> Result<String, AiCoreError> {
    if questions.is_empty() {
        return Err(AiCoreError::ToolExecutionError(
            "No questions provided".to_string(),
        ));
    }

    let formatted: Vec<String> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| format!("{}. {}", i + 1, q))
        .collect();

    Ok(format!(
        "I need some clarification to proceed:\n{}\n\nPlease answer these questions and I'll continue.",
        formatted.join("\n")
    ))
}
