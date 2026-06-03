use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use enigo::{Key, Keyboard};
use serde::Deserialize;

#[derive(Deserialize)]
struct KeyComboArgs {
    #[serde(alias = "combo_str")]
    key_combo: String,
}

/// Action to execute a key combination.
pub struct KeyComboAction;

#[async_trait]
impl ToolAction for KeyComboAction {
    fn tool_name(&self) -> &'static str {
        "app.keyboard_combo"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.keyboard_combo"),
            version: ToolVersion::default(),
            display_name:
                "Simulates a key combination with '+' separator (e.g., 'ctrl+c', 'alt+f4')."
                    .to_string(),
            summary: "Simulates a key combination with '+' separator (e.g., 'ctrl+c', 'alt+f4')."
                .to_string(),
            description:
                "Simulates a key combination with '+' separator (e.g., 'ctrl+c', 'alt+f4')."
                    .to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["keyboard", "combo", "shortcut", "hotkey"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key_combo": { "type": "string", "description": "Key combination with '+' separator (e.g., 'ctrl+shift+s', 'ctrl+c')" }
                },
                "required": ["key_combo"]
            }),
            examples: vec![
                ToolExample {
                    description: "Copy (Ctrl+C)".to_string(),
                    input: serde_json::json!({"key_combo": "ctrl+c"}),
                    output: None,
                },
                ToolExample {
                    description: "Save (Ctrl+S)".to_string(),
                    input: serde_json::json!({"key_combo": "ctrl+s"}),
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
        let args: KeyComboArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let combo = args.key_combo.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Failed to initialize enigo: {e}"),
                }
            })?;

            let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
            if parts.is_empty() {
                return Err(ToolError::ExecutionFailed {
                    message: "Empty key combo".to_string(),
                });
            }

            let keys: Vec<Key> = parts
                .iter()
                .filter_map(|p| crate::utils::parse_key(p))
                .collect();

            if keys.len() != parts.len() {
                let unrecognized: Vec<&&str> = parts
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| crate::utils::parse_key(parts[*i]).is_none())
                    .map(|(_, p)| p)
                    .collect();
                return Err(ToolError::ExecutionFailed {
                    message: format!("Unrecognized key(s) in combo: {:?}", unrecognized),
                });
            }

            let modifier_count = keys.len().saturating_sub(1);
            for key in &keys[..modifier_count] {
                enigo.key(*key, enigo::Direction::Press).map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Key press failed: {e}"),
                    }
                })?;
            }

            if let Some(last) = keys.last() {
                enigo.key(*last, enigo::Direction::Click).map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Key click failed: {e}"),
                    }
                })?;
            }

            for key in keys[..modifier_count].iter().rev() {
                enigo.key(*key, enigo::Direction::Release).map_err(|e| {
                    ToolError::ExecutionFailed {
                        message: format!("Key release failed: {e}"),
                    }
                })?;
            }

            Ok::<_, ToolError>(format!("Executed key combo: {}", combo))
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}
