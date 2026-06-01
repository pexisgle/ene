use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
use ene_tools_common::ToolAction;

/// Action to get the current date and time on the user's system.
pub struct GetCurrentTime;

#[async_trait]
impl ToolAction for GetCurrentTime {
    fn definition(&self) -> ToolDefinition {
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

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let now = chrono::Local::now();
        Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
    }
}
