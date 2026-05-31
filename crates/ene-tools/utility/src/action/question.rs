use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};

/// Returns the `ToolDefinition` for the question tool.
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
        category: Some(ToolCategory::Utility),
        keywords: vec!["question".to_string(), "ask".to_string(), "clarify".to_string(), "confirm".to_string()],
    }
}

/// Asks the user clarifying questions and returns a formatted prompt.
pub fn question(questions: Vec<String>) -> Result<String, ToolError> {
    if questions.is_empty() {
        return Err(ToolError::InvalidArguments {
            message: "No questions provided".to_string(),
        });
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
