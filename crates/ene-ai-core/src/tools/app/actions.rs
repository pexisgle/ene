use crate::error::AiCoreError;
use enigo::{Keyboard, Mouse};

pub async fn list_windows() -> Result<String, AiCoreError> {
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

pub async fn focus_window(title: &str) -> Result<String, AiCoreError> {
    let windows = xcap::Window::all()
        .map_err(|e| AiCoreError::AppError(format!("Failed to enumerate windows: {e}")))?;

    for window in windows {
        let win_title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        if win_title.contains(title) || app_name.contains(title) {
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

pub async fn type_text(text: &str) -> Result<String, AiCoreError> {
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

pub async fn press_key(key: &str) -> Result<String, AiCoreError> {
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| AiCoreError::AppError(format!("Failed to initialize enigo: {e}")))?;

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

pub async fn mouse_move(x: i32, y: i32) -> Result<String, AiCoreError> {
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

pub async fn mouse_click(button: &str) -> Result<String, AiCoreError> {
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

pub async fn clipboard_read() -> Result<String, AiCoreError> {
    Err(AiCoreError::AppError(
        "Clipboard read is not yet implemented. Requires a clipboard library (arboard).".to_string(),
    ))
}

pub async fn clipboard_write(_text: &str) -> Result<String, AiCoreError> {
    Err(AiCoreError::AppError(
        "Clipboard write is not yet implemented. Requires a clipboard library (arboard).".to_string(),
    ))
}
