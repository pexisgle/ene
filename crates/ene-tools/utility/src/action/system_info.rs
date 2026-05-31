use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "get_system_info".to_string(),
        description: "Get basic information about the user's system.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        category: Some(ToolCategory::Utility),
        keywords: vec![
            "system".to_string(),
            "os".to_string(),
            "platform".to_string(),
        ],
    }
}

pub fn get_system_info() -> Result<String, ToolError> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    Ok(format!("OS: {}, Architecture: {}", os, arch))
}
