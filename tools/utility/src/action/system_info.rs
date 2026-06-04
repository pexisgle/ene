use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};

/// Action to get basic information about the user's system.
pub struct GetSystemInfo;

#[async_trait]
impl ToolAction for GetSystemInfo {
    fn tool_name(&self) -> &'static str {
        "utility.get_system_info"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("utility.get_system_info"),
            version: ToolVersion::default(),
            display_name: "Get basic information about the user's system.".to_string(),
            summary: "Get basic information about the user's system.".to_string(),
            description: "Get basic information about the user's system.".to_string(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::primary_only(["system", "os", "platform"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "Get system OS and architecture".to_string(),
                input: serde_json::json!({}),
                output: Some("OS: linux, Architecture: x86_64".to_string()),
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        Ok(format!("OS: {}, Architecture: {}", os, arch))
    }
}
