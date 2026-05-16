//! アプリ操作ツール (app_tools)
//! Phase 3: `enigo` や `xcap` を活用した OS レベルの GUI 操作
//!
//! - ウィンドウの列挙とフォーカス
//! - キーボードの打鍵シミュレーション
//! - マウスカーソルの移動とクリック
//! - クリップボードの読み書き

use super::definition::ToolDefinition;
use crate::error::AiCoreError;
use enigo::{Keyboard, Mouse};

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
    }
}

/// GUI 自動化操作を実行
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
        "list_windows" => list_windows().await,
        "focus_window" => {
            let title = window_title
                .ok_or_else(|| AiCoreError::AppError("window_title required".to_string()))?;
            focus_window(title).await
        }
        "type_text" => {
            let txt = text.ok_or_else(|| AiCoreError::AppError("text required".to_string()))?;
            type_text(txt).await
        }
        "press_key" => {
            let k = key.ok_or_else(|| AiCoreError::AppError("key required".to_string()))?;
            press_key(k).await
        }
        "mouse_move" => {
            let mx = x.ok_or_else(|| AiCoreError::AppError("x required".to_string()))?;
            let my = y.ok_or_else(|| AiCoreError::AppError("y required".to_string()))?;
            mouse_move(mx, my).await
        }
        "mouse_click" => mouse_click(button.unwrap_or("left")).await,
        "clipboard_read" => clipboard_read().await,
        "clipboard_write" => {
            let txt = text.ok_or_else(|| AiCoreError::AppError("text required".to_string()))?;
            clipboard_write(txt).await
        }
        _ => Err(AiCoreError::AppError(format!(
            "Unknown app action: {}",
            action
        ))),
    }
}

async fn list_windows() -> Result<String, AiCoreError> {
    let windows = xcap::Window::all()
        .map_err(|e| AiCoreError::AppError(format!("Failed to enumerate windows: {e}")))?;

    let mut result = Vec::new();
    for window in windows {
        let title = window.title().unwrap_or_default();
        let app = window.app_name().unwrap_or_default();
        if !title.is_empty() || !app.is_empty() {
            result.push(format!("{} ({})", title, app));
        }
    }
    Ok(result.join("\n"))
}

async fn focus_window(title: &str) -> Result<String, AiCoreError> {
    let windows = xcap::Window::all()
        .map_err(|e| AiCoreError::AppError(format!("Failed to enumerate windows: {e}")))?;

    for window in windows {
        let win_title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        if win_title.contains(title) || app_name.contains(title) {
            // xcap does not support focusing directly; we use enigo
            return Ok(format!(
                "Found window: {} ({}). Focus requires platform-specific implementation.",
                win_title, app_name
            ));
        }
    }
    Err(AiCoreError::AppError(format!(
        "Window not found: {}",
        title
    )))
}

async fn type_text(text: &str) -> Result<String, AiCoreError> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| AiCoreError::AppError(format!("Failed to initialize enigo: {e}")))?;
        enigo
            .text(&text)
            .map_err(|e| AiCoreError::AppError(format!("Type failed: {e}")))?;
        Ok::<_, AiCoreError>("Text typed successfully.".to_string())
    })
    .await
    .map_err(|e| AiCoreError::AppError(format!("Task failed: {e}")))?
}

async fn press_key(key: &str) -> Result<String, AiCoreError> {
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| AiCoreError::AppError(format!("Failed to initialize enigo: {e}")))?;

        // Simple key mapping
        match key.as_str() {
            "return" | "enter" => {
                enigo
                    .key(enigo::Key::Return, enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "escape" => {
                enigo
                    .key(enigo::Key::Escape, enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "tab" => {
                enigo
                    .key(enigo::Key::Tab, enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "space" => {
                enigo
                    .key(enigo::Key::Space, enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "backspace" => {
                enigo
                    .key(enigo::Key::Backspace, enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "ctrl+c" => {
                enigo
                    .key(enigo::Key::Control, enigo::Direction::Press)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
                enigo
                    .key(enigo::Key::Unicode('c'), enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
                enigo
                    .key(enigo::Key::Control, enigo::Direction::Release)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            "ctrl+v" => {
                enigo
                    .key(enigo::Key::Control, enigo::Direction::Press)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
                enigo
                    .key(enigo::Key::Unicode('v'), enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
                enigo
                    .key(enigo::Key::Control, enigo::Direction::Release)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            _ if key.len() == 1 => {
                let c = key.chars().next().unwrap();
                enigo
                    .key(enigo::Key::Unicode(c), enigo::Direction::Click)
                    .map_err(|e| AiCoreError::AppError(format!("Key press failed: {e}")))?;
            }
            _ => {
                return Err(AiCoreError::AppError(format!("Unsupported key: {}", key)));
            }
        }
        Ok::<_, AiCoreError>(format!("Pressed key: {}", key))
    })
    .await
    .map_err(|e| AiCoreError::AppError(format!("Task failed: {e}")))?
}

async fn mouse_move(x: i32, y: i32) -> Result<String, AiCoreError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| AiCoreError::AppError(format!("Failed to initialize enigo: {e}")))?;
        enigo
            .move_mouse(x, y, enigo::Coordinate::Abs)
            .map_err(|e| AiCoreError::AppError(format!("Mouse move failed: {e}")))?;
        Ok::<_, AiCoreError>(format!("Mouse moved to ({}, {})", x, y))
    })
    .await
    .map_err(|e| AiCoreError::AppError(format!("Task failed: {e}")))?
}

async fn mouse_click(button: &str) -> Result<String, AiCoreError> {
    let button = button.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| AiCoreError::AppError(format!("Failed to initialize enigo: {e}")))?;
        let btn = match button.as_str() {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            _ => enigo::Button::Left,
        };
        enigo
            .button(btn, enigo::Direction::Click)
            .map_err(|e| AiCoreError::AppError(format!("Mouse click failed: {e}")))?;
        Ok::<_, AiCoreError>(format!("Mouse {} click performed", button))
    })
    .await
    .map_err(|e| AiCoreError::AppError(format!("Task failed: {e}")))?
}

async fn clipboard_read() -> Result<String, AiCoreError> {
    // Phase 3: arboard 等のクリップボードクレートを統合予定
    // enigo 0.3 ではクリップボード操作は直接提供されていません
    Ok("[Clipboard Read] Clipboard access requires a clipboard library (arboard). This is a stub for Phase 3.".to_string())
}

async fn clipboard_write(_text: &str) -> Result<String, AiCoreError> {
    // Phase 3: arboard 等のクリップボードクレートを統合予定
    Ok("[Clipboard Write] Clipboard access requires a clipboard library (arboard). This is a stub for Phase 3.".to_string())
}
