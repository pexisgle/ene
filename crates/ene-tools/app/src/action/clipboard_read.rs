use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;

/// Action to read text from the clipboard.
pub struct ClipboardReadAction;

#[async_trait]
impl ToolAction for ClipboardReadAction {
    fn tool_name(&self) -> &'static str {
        "app.clipboard_read"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.clipboard_read"),
            version: ToolVersion::default(),
            display_name: "Reads current text from the system clipboard.".to_string(),
            summary: "Reads current text from the system clipboard.".to_string(),
            description: "Reads current text from the system clipboard.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["clipboard", "read", "paste"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            examples: vec![ToolExample {
                description: "Read text from clipboard".to_string(),
                input: serde_json::json!({}),
                output: Some("copied text content".to_string()),
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, _arguments: &str) -> Result<String, ToolError> {
        tokio::task::spawn_blocking(move || {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to access clipboard: {e}"),
                })?;
            let content = clipboard
                .get_text()
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to read clipboard: {e}"),
                })?;
            Ok::<_, ToolError>(content)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
