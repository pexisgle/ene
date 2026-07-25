use ene_tool_sdk::prelude::*;
use enigo::Keyboard;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "type_text",
    summary = "Simulates keyboard typing of a string character by character.",
    description = "Simulates keyboard typing of a string character by character.",
    category = "App",
    keywords_primary = "keyboard, type, input, text",
    side_effects = "System { privileged: true }"
)]
pub struct TypeTextAction {
    /// Text to type.
    text: String,
}

impl TypeTextAction {
    async fn run(&self) -> Result<String, ToolError> {
        let text = self.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::execution_failed(format!("Failed to initialize enigo: {e}"))
            })?;
            enigo
                .text(&text)
                .map_err(|e| ToolError::execution_failed(format!("Type failed: {e}")))?;
            Ok::<_, ToolError>("Text typed successfully.".to_string())
        })
        .await
        .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
    }
}
