use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;

/// Action to get the current date and time on the user's system.
pub struct GetCurrentTime;

#[async_trait]
impl ToolAction for GetCurrentTime {
    fn tool_name(&self) -> &'static str {
        "utility.get_current_time"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("utility.get_current_time"),
            version: ToolVersion::default(),
            display_name: "Get the current date and time on the user's system.".to_string(),
            summary: "Get the current date and time on the user's system.".to_string(),
            description: "Get the current date and time on the user's system.".to_string(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::primary_only(["time", "date"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "Get current date and time".to_string(),
                input: serde_json::json!({}),
                output: Some("2026-06-03 14:30:00".to_string()),
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let now = chrono::Local::now();
        Ok(now.format("%Y-%m-%d %H:%M:%S").to_string())
    }
}
