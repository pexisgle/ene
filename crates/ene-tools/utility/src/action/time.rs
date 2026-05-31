use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "get_current_time".to_string(),
        description: "Get the current date and time on the user's system.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: Some(ToolCategory::Utility),
        keywords: vec!["time".to_string(), "date".to_string()],
    }
}

pub fn get_current_time() -> Result<String, ToolError> {
    let now = chrono::Local::now();
    Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
}
