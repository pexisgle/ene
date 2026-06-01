//! Static configuration for the app tool: schema and defaults.

use ene_tool_proto::{ToolCategory, ToolDefinition};

/// Default scale percentage used when callers omit `scale_percent`.
pub const DEFAULT_SCALE_PERCENT: u32 = 50;

/// Returns the `ToolDefinition` for the app tool.
pub fn app_tool_definition() -> ToolDefinition {
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
