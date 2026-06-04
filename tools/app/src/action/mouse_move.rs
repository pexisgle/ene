use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use enigo::{Coordinate, Mouse};
use serde::Deserialize;

#[derive(Deserialize)]
struct MouseMoveArgs {
    x: i32,
    y: i32,
    #[serde(default)]
    relative: Option<bool>,
}

async fn run(x: i32, y: i32, relative: bool) -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        let coord = if relative {
            Coordinate::Rel
        } else {
            Coordinate::Abs
        };
        enigo
            .move_mouse(x, y, coord)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;
        let mode = if relative { "relative" } else { "absolute" };
        Ok::<_, ToolError>(format!("Mouse moved to ({}, {}) [{}]", x, y, mode))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

/// Action to move the mouse cursor.
pub struct MouseMoveAction;

#[async_trait]
impl ToolAction for MouseMoveAction {
    fn tool_name(&self) -> &'static str {
        "app.mouse_move"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.mouse_move"),
            version: ToolVersion::default(),
            display_name: "Moves the mouse cursor to absolute or relative coordinates.".to_string(),
            summary: "Moves the mouse cursor to absolute or relative coordinates.".to_string(),
            description: "Moves the mouse cursor to absolute or relative coordinates.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["mouse", "move", "cursor"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "description": "X coordinate" },
                    "y": { "type": "integer", "description": "Y coordinate" },
                    "relative": { "type": "boolean", "description": "If true, move relative to current position (default: false)" }
                },
                "required": ["x", "y"]
            }),
            examples: vec![
                ToolExample {
                    description: "Move mouse to absolute position".to_string(),
                    input: serde_json::json!({"x": 100, "y": 200}),
                    output: None,
                },
                ToolExample {
                    description: "Move mouse relative by offset".to_string(),
                    input: serde_json::json!({"x": 50, "y": 0, "relative": true}),
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
        let args: MouseMoveArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;
        run(args.x, args.y, args.relative.unwrap_or(false)).await
    }
}
