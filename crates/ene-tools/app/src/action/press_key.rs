use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use enigo::Keyboard;
use serde::Deserialize;

#[derive(Deserialize)]
struct PressKeyArgs {
    key: String,
}

/// Action to press a single key.
pub struct PressKeyAction;

#[async_trait]
impl ToolAction for PressKeyAction {
    fn tool_name(&self) -> &'static str {
        "app.press_key"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.keyboard_press"),
            version: ToolVersion::default(),
            display_name: "Simulates pressing and releasing a single key.".to_string(),
            summary: "Simulates pressing and releasing a single key.".to_string(),
            description: "Simulates pressing and releasing a single key.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["keyboard", "press", "key"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Key name (e.g., 'return', 'escape', 'tab', 'space', 'up', 'down', 'f1'-'f12')" }
                },
                "required": ["key"]
            }),
            examples: vec![
                ToolExample {
                    description: "Press Enter key".to_string(),
                    input: serde_json::json!({"key": "return"}),
                    output: None,
                },
                ToolExample {
                    description: "Press Escape key".to_string(),
                    input: serde_json::json!({"key": "escape"}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: PressKeyArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let key = args.key.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Failed to initialize enigo: {e}"),
                }
            })?;

            if let Some(enigo_key) = crate::utils::parse_key(&key) {
                enigo.key(enigo_key, enigo::Direction::Click).map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Key press failed: {e}"),
                    }
                })?;
                Ok::<_, ToolError>(format!("Pressed key: {}", key))
            } else {
                Err(ToolError::ExecutionFailed {
                    message: format!("Unsupported key: {}", key),
                })
            }
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
