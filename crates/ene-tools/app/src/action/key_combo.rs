use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError};
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
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "key_combo".to_string(),
            description: "Simulates a key combination with '+' separator.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "key_combo": { "type": "string", "description": "Combo e.g., 'ctrl+c', 'alt+f4'" }
                },
                "required": ["key_combo"]
            }),
            category: Some(ToolCategory::App),
            keywords: vec![
                "keyboard".to_string(),
                "combo".to_string(),
                "shortcut".to_string(),
            ],
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
