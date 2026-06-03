use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use enigo::{Button, Coordinate, Direction, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseDragArgs {
    x: i32,
    y: i32,
    x2: i32,
    y2: i32,
    #[serde(default)]
    button: Option<String>,
}

async fn run(
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    button: &str,
) -> Result<String, ToolError> {
    let button = button.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;

        enigo
            .move_mouse(start_x, start_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        let btn = match button.as_str() {
            "right" => Button::Right,
            "middle" => Button::Middle,
            _ => Button::Left,
        };
        enigo
            .button(btn, Direction::Press)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse press failed: {e}"),
            })?;

        enigo
            .move_mouse(end_x, end_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        enigo
            .button(btn, Direction::Release)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse release failed: {e}"),
            })?;

        Ok::<_, ToolError>(format!(
            "Dragged from ({},{}) to ({},{}) with {} button",
            start_x, start_y, end_x, end_y, button
        ))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to drag the mouse.
pub struct MouseDragAction;

#[async_trait]
impl ToolAction for MouseDragAction {
    fn tool_name(&self) -> &'static str {
        "app.drag"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.drag"),
            version: ToolVersion::default(),
            display_name: "Drags the mouse from one coordinate to another.".to_string(),
            summary: "Drags the mouse from one coordinate to another.".to_string(),
            description: "Drags the mouse from one coordinate to another.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["mouse", "drag", "drop"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "Start X coordinate" },
                    "y": { "type": "integer", "description": "Start Y coordinate" },
                    "x2": { "type": "integer", "description": "End X coordinate" },
                    "y2": { "type": "integer", "description": "End Y coordinate" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button (default: left)" }
                },
                "required": ["x", "y", "x2", "y2"]
            }),
            examples: vec![ToolExample {
                description: "Drag from top-left to bottom-right".to_string(),
                input: serde_json::json!({"x": 0, "y": 0, "x2": 500, "y2": 400}),
                output: None,
            }],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: MouseDragArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(
            args.x,
            args.y,
            args.x2,
            args.y2,
            args.button.as_deref().unwrap_or("left"),
        )
        .await
    }
}
