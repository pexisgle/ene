mod actions;

use super::definition::ToolDefinition;
use crate::error::AiCoreError;

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "app".to_string(),
        description: concat!(
            "Performs OS-level GUI automation using enigo and xcap. ",
            "Supports window enumeration, focus, keyboard input, mouse movement/clicks/drag/scroll, ",
            "clipboard read/write, screenshots, monitor listing, and more. ",
            "Use this when you need to interact with the desktop environment or applications directly."
        ).to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "list_windows",
                        "focus_window",
                        "get_active_window",
                        "list_monitors",
                        "type_text",
                        "press_key",
                        "key_combo",
                        "mouse_move",
                        "mouse_click",
                        "mouse_drag",
                        "mouse_scroll",
                        "screenshot",
                        "capture_window",
                        "clipboard_read",
                        "clipboard_write"
                    ],
                    "description": "The GUI automation action to perform"
                },
                "window_title": { "type": "string", "description": "Window title for focus_window or capture_window (substring match)" },
                "text": { "type": "string", "description": "Text to type or write to clipboard" },
                "key": { "type": "string", "description": "Key to press (e.g., 'return', 'escape', 'tab', 'space', 'f1'-'f12', 'up', 'down', 'left', 'right')" },
                "key_combo": { "type": "string", "description": "Key combination with '+' separator (e.g., 'ctrl+shift+s', 'alt+f4', 'ctrl+c')" },
                "x": { "type": "integer", "description": "X coordinate for mouse_move or mouse_drag start" },
                "y": { "type": "integer", "description": "Y coordinate for mouse_move or mouse_drag start" },
                "x2": { "type": "integer", "description": "End X coordinate for mouse_drag" },
                "y2": { "type": "integer", "description": "End Y coordinate for mouse_drag" },
                "button": { "type": "string", "enum": ["left", "right", "middle", "back", "forward"], "description": "Mouse button" },
                "count": { "type": "integer", "description": "Number of clicks for mouse_click (default: 1, use 2 for double-click)" },
                "relative": { "type": "boolean", "description": "If true, mouse_move coordinates are relative to current position (default: false)" },
                "amount": { "type": "integer", "description": "Scroll amount for mouse_scroll (positive number of steps)" },
                "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "description": "Scroll direction for mouse_scroll" },
                "scale_percent": { "type": "integer", "description": "Screenshot scale percentage (1-100, default: 50). Lower values reduce image size." }
            },
            "required": ["action"]
        }),
        category: Some(super::ToolCategory::App),
        keywords: vec![
            "gui".to_string(),
            "automation".to_string(),
            "mouse".to_string(),
            "keyboard".to_string(),
            "clipboard".to_string(),
            "window".to_string(),
            "screenshot".to_string(),
            "screen".to_string(),
        ],
    }
}

pub async fn app_exec(
    action: &str,
    window_title: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    key_combo: Option<&str>,
    x: Option<i32>,
    y: Option<i32>,
    x2: Option<i32>,
    y2: Option<i32>,
    button: Option<&str>,
    count: Option<u32>,
    relative: Option<bool>,
    amount: Option<i32>,
    direction: Option<&str>,
    scale_percent: Option<u32>,
) -> Result<String, AiCoreError> {
    match action {
        "list_windows" => actions::list_windows().await,
        "list_monitors" => actions::list_monitors().await,
        "get_active_window" => actions::get_active_window().await,
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
        "key_combo" => {
            let combo = key_combo.ok_or_else(|| AiCoreError::AppError("key_combo required".to_string()))?;
            actions::key_combo(combo).await
        }
        "mouse_move" => {
            let mx = x.ok_or_else(|| AiCoreError::AppError("x required".to_string()))?;
            let my = y.ok_or_else(|| AiCoreError::AppError("y required".to_string()))?;
            actions::mouse_move(mx, my, relative.unwrap_or(false)).await
        }
        "mouse_click" => actions::mouse_click(button.unwrap_or("left"), count).await,
        "mouse_drag" => {
            let sx = x.ok_or_else(|| AiCoreError::AppError("x (start_x) required".to_string()))?;
            let sy = y.ok_or_else(|| AiCoreError::AppError("y (start_y) required".to_string()))?;
            let ex = x2.ok_or_else(|| AiCoreError::AppError("x2 (end_x) required".to_string()))?;
            let ey = y2.ok_or_else(|| AiCoreError::AppError("y2 (end_y) required".to_string()))?;
            actions::mouse_drag(sx, sy, ex, ey, button.unwrap_or("left")).await
        }
        "mouse_scroll" => {
            let amt = amount.ok_or_else(|| AiCoreError::AppError("amount required".to_string()))?;
            let dir = direction.unwrap_or("down");
            actions::mouse_scroll(amt, dir).await
        }
        "screenshot" => actions::screenshot(scale_percent).await,
        "capture_window" => {
            let title = window_title
                .ok_or_else(|| AiCoreError::AppError("window_title required".to_string()))?;
            actions::capture_window(title, scale_percent).await
        }
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