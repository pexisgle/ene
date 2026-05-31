mod clipboard;
mod input;
mod mouse;
mod screenshot;
mod window;

use ene_tool_proto::ToolError;

pub async fn app_exec(
    action: &str,
    window_title: Option<&str>,
    text: Option<&str>,
    key: Option<&str>,
    combo_str: Option<&str>,
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
) -> Result<String, ToolError> {
    match action {
        "list_windows" => window::list_windows().await,
        "list_monitors" => window::list_monitors().await,
        "get_active_window" => window::get_active_window().await,
        "focus_window" => {
            let title = window_title.ok_or_else(|| ToolError::InvalidArguments {
                message: "window_title required".to_string(),
            })?;
            window::focus_window(title).await
        }
        "type_text" => {
            let txt = text.ok_or_else(|| ToolError::InvalidArguments {
                message: "text required".to_string(),
            })?;
            input::type_text(txt).await
        }
        "press_key" => {
            let k = key.ok_or_else(|| ToolError::InvalidArguments {
                message: "key required".to_string(),
            })?;
            input::press_key(k).await
        }
        "key_combo" => {
            let combo = combo_str.ok_or_else(|| ToolError::InvalidArguments {
                message: "key_combo required".to_string(),
            })?;
            input::key_combo(combo).await
        }
        "mouse_move" => {
            let mx = x.ok_or_else(|| ToolError::InvalidArguments {
                message: "x required".to_string(),
            })?;
            let my = y.ok_or_else(|| ToolError::InvalidArguments {
                message: "y required".to_string(),
            })?;
            mouse::mouse_move(mx, my, relative.unwrap_or(false)).await
        }
        "mouse_click" => mouse::mouse_click(button.unwrap_or("left"), count).await,
        "mouse_drag" => {
            let sx = x.ok_or_else(|| ToolError::InvalidArguments {
                message: "x (start_x) required".to_string(),
            })?;
            let sy = y.ok_or_else(|| ToolError::InvalidArguments {
                message: "y (start_y) required".to_string(),
            })?;
            let ex = x2.ok_or_else(|| ToolError::InvalidArguments {
                message: "x2 (end_x) required".to_string(),
            })?;
            let ey = y2.ok_or_else(|| ToolError::InvalidArguments {
                message: "y2 (end_y) required".to_string(),
            })?;
            mouse::mouse_drag(sx, sy, ex, ey, button.unwrap_or("left")).await
        }
        "mouse_scroll" => {
            let amt = amount.ok_or_else(|| ToolError::InvalidArguments {
                message: "amount required".to_string(),
            })?;
            let dir = direction.unwrap_or("down");
            mouse::mouse_scroll(amt, dir).await
        }
        "screenshot" => screenshot::screenshot(scale_percent).await,
        "capture_window" => {
            let title = window_title.ok_or_else(|| ToolError::InvalidArguments {
                message: "window_title required".to_string(),
            })?;
            screenshot::capture_window(title, scale_percent).await
        }
        "clipboard_read" => clipboard::clipboard_read().await,
        "clipboard_write" => {
            let txt = text.ok_or_else(|| ToolError::InvalidArguments {
                message: "text required".to_string(),
            })?;
            clipboard::clipboard_write(txt).await
        }
        _ => Err(ToolError::InvalidArguments {
            message: format!("Unknown app action: {}", action),
        }),
    }
}
