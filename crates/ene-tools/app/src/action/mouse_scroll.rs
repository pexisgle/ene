use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use enigo::{Axis, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseScrollArgs {
    amount: i32,
    #[serde(default)]
    direction: Option<String>,
}

async fn run(amount: i32, direction: &str) -> Result<String, ToolError> {
    let dir_str = direction.to_string();
    let is_horizontal = direction == "left" || direction == "right";
    let scroll_amount = match direction {
        "up" => -amount,
        "down" => amount,
        "left" => -amount,
        "right" => amount,
        _ => amount,
    };
    let axis = if is_horizontal {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };

    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        enigo
            .scroll(scroll_amount, axis)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Scroll failed: {e}"),
            })?;
        Ok::<_, ToolError>(format!("Scrolled {} by {}", dir_str, amount))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to scroll the mouse.
pub struct MouseScrollAction;

#[async_trait]
impl ToolAction for MouseScrollAction {
    fn tool_name(&self) -> &'static str {
        "app.mouse_scroll"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.mouse_scroll"),
            version: ToolVersion::default(),
            display_name: "Simulates mouse scrolling in a given direction.".to_string(),
            summary: "Simulates mouse scrolling in a given direction.".to_string(),
            description: "Simulates mouse scrolling in a given direction.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["mouse", "scroll", "wheel"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "amount": { "type": "integer", "description": "Number of scroll steps" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction (default: down)" }
                },
                "required": ["amount"]
            }),
            examples: vec![
                ToolExample {
                    description: "Scroll down 3 steps".to_string(),
                    input: serde_json::json!({"amount": 3, "direction": "down"}),
                    output: None,
                },
                ToolExample {
                    description: "Scroll up 5 steps".to_string(),
                    input: serde_json::json!({"amount": 5, "direction": "up"}),
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
        let args: MouseScrollArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(args.amount, args.direction.as_deref().unwrap_or("down")).await
    }
}
