use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

/// Action to get basic information about the user's system.
pub struct GetSystemInfo;

#[async_trait]
impl ToolAction for GetSystemInfo {
    fn definition(&self) -> ToolDefinition {
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

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        Ok(format!("OS: {}, Architecture: {}", os, arch))
    }
}
