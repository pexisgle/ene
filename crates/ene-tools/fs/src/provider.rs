use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, ToolProvider};
use async_trait::async_trait;

pub struct FsToolProvider;

#[async_trait]
impl ToolProvider for FsToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "filesystem".to_string(),
                description: "Performs file system operations. Supports read, write, edit, delete, search, patch, and list.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "description": "The filesystem action to perform" }
                    },
                    "required": ["action"]
                }),
                category: Some(ToolCategory::Filesystem),
                keywords: vec!["fs".to_string(), "file".to_string(), "directory".to_string()],
            },
            ToolDefinition {
                name: "shell".to_string(),
                description: "Executes shell commands.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to execute" }
                    },
                    "required": ["command"]
                }),
                category: Some(ToolCategory::Shell),
                keywords: vec!["shell".to_string(), "command".to_string(), "bash".to_string()],
            },
            ToolDefinition {
                name: "undo".to_string(),
                description: "Reverts the most recent file operation.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                category: Some(ToolCategory::Utility),
                keywords: vec!["undo".to_string(), "revert".to_string()],
            },
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "filesystem" | "shell" | "undo" => {
                Err(ToolError::ExecutionFailed {
                    message: format!("Tool '{}' is being migrated to IPC host. Not yet fully implemented in ene-tools-fs.", name),
                })
            }
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {
        // FS tools will use session_id for undo tracking in future
    }
}
