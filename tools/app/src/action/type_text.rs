use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use enigo::Keyboard;
use serde::Deserialize;

#[derive(Deserialize)]
struct TypeTextArgs {
    text: String,
}

/// Action to type text.
pub struct TypeTextAction;

#[async_trait]
impl ToolAction for TypeTextAction {
    fn tool_name(&self) -> &'static str {
        "app.type_text"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.type_text"),
            version: ToolVersion::default(),
            display_name: "Simulates keyboard typing of a string character by character."
                .to_string(),
            summary: "Simulates keyboard typing of a string character by character.".to_string(),
            description: "Simulates keyboard typing of a string character by character."
                .to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["keyboard", "type", "input", "text"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to type" }
                },
                "required": ["text"]
            }),
            examples: vec![ToolExample {
                description: "Type text on the keyboard".to_string(),
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
        let args: TypeTextArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        let text = args.text.clone();
        tokio::task::spawn_blocking(move || {
            let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Failed to initialize enigo: {e}"),
                }
            })?;
            enigo.text(&text).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Type failed: {e}"),
            })?;
            Ok::<_, ToolError>("Text typed successfully.".to_string())
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Task failed: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_name_matches_tool_name() {
        // The provider dispatches via `action.tool_name()`, but the LLM
        // receives the spec's `name` field as the callable identifier.
        // These MUST match or every call resolves to `Tool not found`.
        let def = TypeTextAction.definition();
        assert_eq!(def.name.to_string(), TypeTextAction.tool_name());
    }
}
