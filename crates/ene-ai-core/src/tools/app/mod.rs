mod actions;

use super::definition::ToolDefinition;
use crate::error::AiCoreError;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "app".to_string(),
        description: concat!(
            "Performs OS-level GUI automation using enigo and xcap. ",
            "Supports window enumeration, focus, keyboard input, mouse movement/clicks, and clipboard read/write. ",
            "Use this when you need to interact with the desktop environment or applications directly."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list_windows", "focus_window", "type_text", "press_key", "mouse_move", "mouse_click", "clipboard_read", "clipboard_write"],
                    "description": "The GUI automation action to perform"
                },
                "window_title": { "type": "string", "description": "Window title for focus_window" },
                "text": { "type": "string", "description": "Text to type or write to clipboard" },
                "key": { "type": "string", "description": "Key to press (e.g., 'return', 'escape', 'ctrl+c')" },
                "x": { "type": "integer", "description": "X coordinate for mouse_move" },
                "y": { "type": "integer", "description": "Y coordinate for mouse_move" },
                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Mouse button" }
            },
            "required": ["action"]
        }),
        category: Some(super::ToolCategory::App),
        keywords: vec!["gui".to_string(), "automation".to_string(), "mouse".to_string(), "keyboard".to_string(), "clipboard".to_string(), "window".to_string()],
    }
}

pub async fn app_exec(
    action: &str,
    window_title: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    x: Option<i32>,
    y: Option<i32>,
    button: Option<&str>,
) -> Result<String, AiCoreError> {
    match action {
        "list_windows" => actions::list_windows().await,
        "focus_window" => {
            let title = window_title
                .ok_or_else(|| AiCoreError::AppError("window_title required".to_string()))?;
            actions::focus_window(title).await
        }
        "type_text" => {
            let txt = text.ok_or_else(|| AiCoreError::AppError("text required".to_string()))?;
            actions::type_text(txt).await
        }
        "press_key" => {
            let k = key.ok_or_else(|| AiCoreError::AppError("key required".to_string()))?;
            actions::press_key(k).await
        }
        "mouse_move" => {
            let mx = x.ok_or_else(|| AiCoreError::AppError("x required".to_string()))?;
            let my = y.ok_or_else(|| AiCoreError::AppError("y required".to_string()))?;
            actions::mouse_move(mx, my).await
        }
        "mouse_click" => actions::mouse_click(button.unwrap_or("left")).await,
        "clipboard_read" => actions::clipboard_read().await,
        "clipboard_write" => {
            let txt = text.ok_or_else(|| AiCoreError::AppError("text required".to_string()))?;
            actions::clipboard_write(txt).await
        }
        _ => Err(AiCoreError::AppError(format!(
            "Unknown app action: {}",
            action
        ))),
    }
}
