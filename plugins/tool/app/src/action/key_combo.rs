use ene_tool_sdk::prelude::*;
use enigo::{Key, Keyboard};

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "keyboard_combo",
    summary = "Simulates a key combination with '+' separator (e.g., 'ctrl+c', 'alt+f4').",
    description = "Simulates a key combination with '+' separator (e.g., 'ctrl+c', 'alt+f4').",
    category = "App",
    keywords_primary = "keyboard, combo, shortcut, hotkey",
    side_effects = "System { privileged: true }"
)]
pub struct KeyComboAction {
    /// Key combination with '+' separator (e.g., 'ctrl+shift+s', 'ctrl+c').
    key_combo: String,
}

impl KeyComboAction {
    async fn run(&self) -> Result<String, ToolError> {
        let combo = self.key_combo.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::execution_failed(format!("Failed to initialize enigo: {e}"))
            })?;

            let parts: Vec<&str> = combo.split('+').map(str::trim).collect();
            if parts.is_empty() {
                return Err(ToolError::execution_failed("Empty key combo".to_string()));
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
                return Err(ToolError::execution_failed(format!(
                    "Unrecognized key(s) in combo: {unrecognized:?}"
                )));
            }

            let modifier_count = keys.len().saturating_sub(1);
            for key in &keys[..modifier_count] {
                enigo
                    .key(*key, enigo::Direction::Press)
                    .map_err(|e| ToolError::execution_failed(format!("Key press failed: {e}")))?;
            }

            if let Some(last) = keys.last() {
                enigo
                    .key(*last, enigo::Direction::Click)
                    .map_err(|e| ToolError::execution_failed(format!("Key click failed: {e}")))?;
            }

            for key in keys[..modifier_count].iter().rev() {
                enigo
                    .key(*key, enigo::Direction::Release)
                    .map_err(|e| ToolError::execution_failed(format!("Key release failed: {e}")))?;
            }

            Ok::<_, ToolError>(format!("Executed key combo: {combo}"))
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
