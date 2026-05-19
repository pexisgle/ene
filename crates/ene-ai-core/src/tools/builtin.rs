use super::definition::{ToolDefinition, ToolRegistry};
use async_trait::async_trait;
use chrono::Local;

#[derive(Default)]
pub struct BuiltinToolRegistry;

impl BuiltinToolRegistry {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolRegistry for BuiltinToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "get_current_time".to_string(),
                description: "Get the current date and time on the user's system.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some(super::ToolCategory::Utility),
                keywords: vec!["time".to_string(), "date".to_string()],
            },
            ToolDefinition {
                name: "get_system_info".to_string(),
                description: "Get basic information about the user's system.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some(super::ToolCategory::Utility),
                keywords: vec!["system".to_string(), "os".to_string(), "platform".to_string()],
            },
        ]
    }

    async fn call_tool(&self, name: &str, _arguments: &str) -> Result<String, String> {
        match name {
            "get_current_time" => {
                let now = Local::now();
                Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
            }
            "get_system_info" => {
                let os = std::env::consts::OS;
                let arch = std::env::consts::ARCH;
                Ok(format!("OS: {}, Architecture: {}", os, arch))
            }
            _ => Err(format!("Unknown built-in tool: {}", name)),
        }
    }
}
