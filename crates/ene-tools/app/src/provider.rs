use crate::actions;
use async_trait::async_trait;
use ene_tool_proto::{ToolCategory, ToolDefinition, ToolError, ToolProvider};
use serde::Deserialize;

pub struct AppToolProvider;

#[derive(Deserialize)]
struct AppArgs {
    action: String,
    #[serde(default)]
    window_title: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    combo_str: Option<String>,
    #[serde(default)]
    x: Option<i32>,
    #[serde(default)]
    y: Option<i32>,
    #[serde(default)]
    x2: Option<i32>,
    #[serde(default)]
    y2: Option<i32>,
    #[serde(default)]
    button: Option<String>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    relative: Option<bool>,
    #[serde(default)]
    amount: Option<i32>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    scale_percent: Option<u32>,
}

pub fn tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "app".to_string(),
        description: concat!(
            "Performs OS-level GUI automation using enigo, xcap, and xdg-desktop-portal (Wayland). ",
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
        category: Some(ToolCategory::App),
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

#[async_trait]
impl ToolProvider for AppToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![tool_definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        if name != "app" {
            return Err(ToolError::NotFound {
                tool_name: name.to_string(),
            });
        }
        let args: AppArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments for app: {e}"),
            })?;
        actions::app_exec(
            &args.action,
            args.window_title.as_deref(),
            args.text.as_deref(),
            args.key.as_deref(),
            args.combo_str.as_deref(),
            args.x,
            args.y,
            args.x2,
            args.y2,
            args.button.as_deref(),
            args.count,
            args.relative,
            args.amount,
            args.direction.as_deref(),
            args.scale_percent,
        )
        .await
    }

    fn set_session_id(&self, _session_id: &str) {
        // App tools are stateless
    }
}
