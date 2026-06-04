use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct ClipboardWriteArgs {
    text: String,
}

/// Action to write text to the clipboard.
pub struct ClipboardWriteAction;

#[async_trait]
impl ToolAction for ClipboardWriteAction {
    fn tool_name(&self) -> &'static str {
        "app.clipboard_write"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.clipboard_write"),
            version: ToolVersion::default(),
            display_name: "Writes text to the system clipboard.".to_string(),
            summary: "Writes text to the system clipboard.".to_string(),
            description: "Writes text to the system clipboard.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["clipboard", "write", "copy"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to write to clipboard" }
                },
                "required": ["text"]
            }),
            examples: vec![ToolExample {
                description: "Write text to clipboard".to_string(),
                input: serde_json::json!({"text": "Hello, world!"}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: ClipboardWriteArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let text = args.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut clipboard =
                arboard::Clipboard::new().map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to access clipboard: {e}"),
                })?;
            clipboard
                .set_text(&text)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to write clipboard: {e}"),
                })?;
            Ok::<_, ToolError>("Clipboard updated.".to_string())
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
