use ene_tool_proto::ToolError;
use base64::{Engine as _, engine::general_purpose};
use enigo::{Axis, Coordinate, Direction, Keyboard, Mouse};
use image::{DynamicImage, imageops::FilterType};
use std::io::Cursor;

use crate::portal;

pub async fn list_windows() -> Result<String, ToolError> {
    if portal::detect_wayland() {
        return portal::list_windows_wayland();
    }

    let windows = xcap::Window::all()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to enumerate windows: {e}") })?;

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

pub async fn focus_window(title: &str) -> Result<String, ToolError> {
    if portal::detect_wayland() {
        return portal::focus_window_wayland(title);
    }

    let windows = xcap::Window::all()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to enumerate windows: {e}") })?;

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
    Err(ToolError::ExecutionFailed { message: format!("Window not found: {}", title) })
}

pub async fn type_text(text: &str) -> Result<String, ToolError> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;
        enigo
            .text(&text)
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Type failed: {e}") })?;
        Ok::<_, ToolError>("Text typed successfully.".to_string())
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn press_key(key: &str) -> Result<String, ToolError> {
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;

        match key.as_str() {
            "return" | "enter" => {
                enigo.key(enigo::Key::Return, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "escape" => {
                enigo.key(enigo::Key::Escape, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "tab" => {
                enigo.key(enigo::Key::Tab, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "space" => {
                enigo.key(enigo::Key::Space, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "backspace" => {
                enigo.key(enigo::Key::Backspace, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "delete" => {
                enigo.key(enigo::Key::Delete, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "home" => {
                enigo.key(enigo::Key::Home, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "end" => {
                enigo.key(enigo::Key::End, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "pageup" => {
                enigo.key(enigo::Key::PageUp, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "pagedown" => {
                enigo.key(enigo::Key::PageDown, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "up" | "arrow_up" => {
                enigo.key(enigo::Key::UpArrow, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "down" | "arrow_down" => {
                enigo.key(enigo::Key::DownArrow, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "left" | "arrow_left" => {
                enigo.key(enigo::Key::LeftArrow, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "right" | "arrow_right" => {
                enigo.key(enigo::Key::RightArrow, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "insert" => {
                enigo.key(enigo::Key::Insert, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "capslock" => {
                enigo.key(enigo::Key::CapsLock, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            "f1" => { enigo.key(enigo::Key::F1, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f2" => { enigo.key(enigo::Key::F2, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f3" => { enigo.key(enigo::Key::F3, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f4" => { enigo.key(enigo::Key::F4, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f5" => { enigo.key(enigo::Key::F5, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f6" => { enigo.key(enigo::Key::F6, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f7" => { enigo.key(enigo::Key::F7, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f8" => { enigo.key(enigo::Key::F8, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f9" => { enigo.key(enigo::Key::F9, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f10" => { enigo.key(enigo::Key::F10, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f11" => { enigo.key(enigo::Key::F11, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            "f12" => { enigo.key(enigo::Key::F12, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?; }
            _ if key.len() == 1 => {
                let c = key.chars().next().unwrap();
                enigo.key(enigo::Key::Unicode(c), Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
            }
            _ => {
                return Err(ToolError::ExecutionFailed { message: format!("Unsupported key: {}", key) });
            }
        }
        Ok::<_, ToolError>(format!("Pressed key: {}", key))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

fn parse_key(name: &str) -> Option<enigo::Key> {
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => Some(enigo::Key::Control),
        "shift" => Some(enigo::Key::Shift),
        "alt" | "option" => Some(enigo::Key::Alt),
        "meta" | "super" | "cmd" | "command" | "win" | "windows" => Some(enigo::Key::Meta),
        "return" | "enter" => Some(enigo::Key::Return),
        "escape" | "esc" => Some(enigo::Key::Escape),
        "tab" => Some(enigo::Key::Tab),
        "space" => Some(enigo::Key::Space),
        "backspace" => Some(enigo::Key::Backspace),
        "delete" | "del" => Some(enigo::Key::Delete),
        "home" => Some(enigo::Key::Home),
        "end" => Some(enigo::Key::End),
        "pageup" | "page_up" => Some(enigo::Key::PageUp),
        "pagedown" | "page_down" => Some(enigo::Key::PageDown),
        "up" | "arrow_up" => Some(enigo::Key::UpArrow),
        "down" | "arrow_down" => Some(enigo::Key::DownArrow),
        "left" | "arrow_left" => Some(enigo::Key::LeftArrow),
        "right" | "arrow_right" => Some(enigo::Key::RightArrow),
        "insert" | "ins" => Some(enigo::Key::Insert),
        "capslock" | "caps_lock" => Some(enigo::Key::CapsLock),
        "f1" => Some(enigo::Key::F1),
        "f2" => Some(enigo::Key::F2),
        "f3" => Some(enigo::Key::F3),
        "f4" => Some(enigo::Key::F4),
        "f5" => Some(enigo::Key::F5),
        "f6" => Some(enigo::Key::F6),
        "f7" => Some(enigo::Key::F7),
        "f8" => Some(enigo::Key::F8),
        "f9" => Some(enigo::Key::F9),
        "f10" => Some(enigo::Key::F10),
        "f11" => Some(enigo::Key::F11),
        "f12" => Some(enigo::Key::F12),
        s if s.len() == 1 => Some(enigo::Key::Unicode(s.chars().next().unwrap())),
        _ => None,
    }
}

pub async fn key_combo(combo: &str) -> Result<String, ToolError> {
    let combo = combo.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;

        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
        if parts.is_empty() {
            return Err(ToolError::ExecutionFailed { message: "Empty key combo".to_string() });
        }

        let keys: Vec<enigo::Key> = parts.iter().filter_map(|p| parse_key(p)).collect();

        if keys.len() != parts.len() {
            let unrecognized: Vec<&&str> = parts.iter().enumerate().filter(|(i, _)| parse_key(parts[*i]).is_none()).map(|(_, p)| p).collect();
            return Err(ToolError::ExecutionFailed { message: format!("Unrecognized key(s) in combo: {:?}", unrecognized) });
        }

        let modifier_count = keys.len().saturating_sub(1);
        for key in &keys[..modifier_count] {
            enigo.key(*key, Direction::Press).map_err(|e| ToolError::ExecutionFailed { message: format!("Key press failed: {e}") })?;
        }

        if let Some(last) = keys.last() {
            enigo.key(*last, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Key click failed: {e}") })?;
        }

        for key in keys[..modifier_count].iter().rev() {
            enigo.key(*key, Direction::Release).map_err(|e| ToolError::ExecutionFailed { message: format!("Key release failed: {e}") })?;
        }

        Ok::<_, ToolError>(format!("Executed key combo: {}", combo))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn mouse_move(x: i32, y: i32, relative: bool) -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;
        let coord = if relative { Coordinate::Rel } else { Coordinate::Abs };
        enigo.move_mouse(x, y, coord).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse move failed: {e}") })?;
        let mode = if relative { "relative" } else { "absolute" };
        Ok::<_, ToolError>(format!("Mouse moved to ({}, {}) [{}]", x, y, mode))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn mouse_click(button: &str, count: Option<u32>) -> Result<String, ToolError> {
    let button = button.to_string();
    let count = count.unwrap_or(1);
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;
        let btn = match button.as_str() {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            "back" => enigo::Button::Back,
            "forward" => enigo::Button::Forward,
            _ => enigo::Button::Left,
        };
        for _ in 0..count {
            enigo.button(btn, Direction::Click).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse click failed: {e}") })?;
        }
        Ok::<_, ToolError>(format!("Mouse {} click x{}", button, count))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn mouse_drag(start_x: i32, start_y: i32, end_x: i32, end_y: i32, button: &str) -> Result<String, ToolError> {
    let button = button.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;

        enigo.move_mouse(start_x, start_y, Coordinate::Abs).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse move failed: {e}") })?;

        let btn = match button.as_str() {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            _ => enigo::Button::Left,
        };
        enigo.button(btn, Direction::Press).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse press failed: {e}") })?;

        enigo.move_mouse(end_x, end_y, Coordinate::Abs).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse move failed: {e}") })?;

        enigo.button(btn, Direction::Release).map_err(|e| ToolError::ExecutionFailed { message: format!("Mouse release failed: {e}") })?;

        Ok::<_, ToolError>(format!("Dragged from ({},{}) to ({},{}) with {} button", start_x, start_y, end_x, end_y, button))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn mouse_scroll(amount: i32, direction: &str) -> Result<String, ToolError> {
    let dir_str = direction.to_string();
    let is_horizontal = direction == "left" || direction == "right";
    let scroll_amount = match direction {
        "up" => -amount,
        "down" => amount,
        "left" => -amount,
        "right" => amount,
        _ => amount,
    };
    let axis = if is_horizontal { Axis::Horizontal } else { Axis::Vertical };

    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default())
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to initialize enigo: {e}") })?;
        enigo.scroll(scroll_amount, axis).map_err(|e| ToolError::ExecutionFailed { message: format!("Scroll failed: {e}") })?;
        Ok::<_, ToolError>(format!("Scrolled {} by {}", dir_str, amount))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn get_active_window() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let active_win = active_win_pos_rs::get_active_window()
            .map_err(|_| ToolError::ExecutionFailed { message: "Failed to get active window".to_string() })?;
        Ok::<_, ToolError>(format!(
            "Active window: {} ({}) at ({}, {}) size {}x{}",
            active_win.title,
            active_win.app_name,
            active_win.position.x,
            active_win.position.y,
            active_win.position.width,
            active_win.position.height,
        ))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn list_monitors() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let monitors = xcap::Monitor::all()
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to enumerate monitors: {e}") })?;

        let mut result = Vec::new();
        for monitor in monitors {
            let name = monitor.name().unwrap_or_else(|_| "Unknown".to_string());
            let id = monitor.id();
            let id_str = match id { Ok(v) => v.to_string(), Err(_) => "?".to_string() };
            let is_primary = monitor.is_primary().unwrap_or(false);
            let width = monitor.width().unwrap_or(0);
            let height = monitor.height().unwrap_or(0);
            let x = monitor.x().unwrap_or(0);
            let y = monitor.y().unwrap_or(0);
            let scale = monitor.scale_factor().unwrap_or(1.0);
            result.push(format!(
                "{} (id: {}) {}x{} at ({},{}) scale={:.1}{}",
                name,
                id_str,
                width,
                height,
                x,
                y,
                scale,
                if is_primary { " [PRIMARY]" } else { "" }
            ));
        }
        Ok::<_, ToolError>(result.join("\n"))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

fn capture_screen_xcap(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    let mut target_image = None;

    if let Ok(active_win) = active_win_pos_rs::get_active_window() {
        if let Ok(windows) = xcap::Window::all() {
            for window in windows {
                let title = window.title().unwrap_or_default();
                let app_name = window.app_name().unwrap_or_default();
                if title == active_win.title || app_name == active_win.app_name {
                    if !window.is_minimized().unwrap_or(false) {
                        if let Ok(img) = window.capture_image() {
                            target_image = Some(DynamicImage::ImageRgba8(img));
                            break;
                        }
                    }
                }
            }
        }
    }

    if target_image.is_none() {
        if let Ok(monitors) = xcap::Monitor::all() {
            let primary = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).unwrap_or_else(|| &monitors[0]);
            if let Ok(img) = primary.capture_image() {
                target_image = Some(DynamicImage::ImageRgba8(img));
            }
        }
    }

    let image = target_image.ok_or_else(|| ToolError::ExecutionFailed { message: "Failed to capture screen".to_string() })?;

    let final_image = if scale_percent > 0 && scale_percent < 100 {
        let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
        let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
        image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
    } else {
        image
    };

    Ok(final_image)
}

fn capture_window_by_title_xcap(title: &str, scale_percent: u32) -> Result<DynamicImage, ToolError> {
    let windows = xcap::Window::all()
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to enumerate windows: {e}") })?;

    for window in windows {
        let win_title = window.title().unwrap_or_default();
        let app_name = window.app_name().unwrap_or_default();
        let is_minimized = window.is_minimized().unwrap_or(false);
        if (win_title.contains(title) || app_name.contains(title)) && !is_minimized {
            if let Ok(img) = window.capture_image() {
                let image = DynamicImage::ImageRgba8(img);
                let final_image = if scale_percent > 0 && scale_percent < 100 {
                    let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
                    let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
                    image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
                } else {
                    image
                };
                return Ok(final_image);
            }
        }
    }

    Err(ToolError::ExecutionFailed { message: format!("Window not found: {}", title) })
}

fn encode_image_to_data_uri(image: DynamicImage) -> Result<String, ToolError> {
    let mut buf = Cursor::new(Vec::new());
    image.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to encode image to PNG: {}", e) })?;
    let b64 = general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{}", b64))
}

pub async fn screenshot(scale_percent: Option<u32>) -> Result<String, ToolError> {
    let scale_percent = scale_percent.unwrap_or(50);

    let image = if portal::detect_wayland() {
        portal::capture_screen_portal(scale_percent).await
    } else {
        let sp = scale_percent;
        tokio::task::spawn_blocking(move || capture_screen_xcap(sp))
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
    }?;

    let data_uri = encode_image_to_data_uri(image)?;
    let result = serde_json::json!({
        "type": "screenshot",
        "data": data_uri
    });
    Ok(result.to_string())
}

pub async fn capture_window(title: &str, scale_percent: Option<u32>) -> Result<String, ToolError> {
    let scale_percent = scale_percent.unwrap_or(50);

    let image = if portal::detect_wayland() {
        portal::capture_window_portal(scale_percent).await
    } else {
        let t = title.to_string();
        let sp = scale_percent;
        tokio::task::spawn_blocking(move || capture_window_by_title_xcap(&t, sp))
            .await
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
    }?;

    let data_uri = encode_image_to_data_uri(image)?;
    let result = serde_json::json!({
        "type": "screenshot",
        "data": data_uri,
        "window": title
    });
    Ok(result.to_string())
}

pub async fn clipboard_read() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to access clipboard: {e}") })?;
        let content = clipboard.get_text()
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to read clipboard: {e}") })?;
        Ok::<_, ToolError>(content)
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

pub async fn clipboard_write(text: &str) -> Result<String, ToolError> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut clipboard = arboard::Clipboard::new()
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to access clipboard: {e}") })?;
        clipboard.set_text(&text)
            .map_err(|e| ToolError::ExecutionFailed { message: format!("Failed to write clipboard: {e}") })?;
        Ok::<_, ToolError>("Clipboard updated.".to_string())
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed { message: format!("Task failed: {e}") })?
}

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
        "list_windows" => list_windows().await,
        "list_monitors" => list_monitors().await,
        "get_active_window" => get_active_window().await,
        "focus_window" => {
            let title = window_title.ok_or_else(|| ToolError::InvalidArguments { message: "window_title required".to_string() })?;
            focus_window(title).await
        }
        "type_text" => {
            let txt = text.ok_or_else(|| ToolError::InvalidArguments { message: "text required".to_string() })?;
            type_text(txt).await
        }
        "press_key" => {
            let k = key.ok_or_else(|| ToolError::InvalidArguments { message: "key required".to_string() })?;
            press_key(k).await
        }
        "key_combo" => {
            let combo = combo_str.ok_or_else(|| ToolError::InvalidArguments { message: "key_combo required".to_string() })?;
            key_combo(combo).await
        }
        "mouse_move" => {
            let mx = x.ok_or_else(|| ToolError::InvalidArguments { message: "x required".to_string() })?;
            let my = y.ok_or_else(|| ToolError::InvalidArguments { message: "y required".to_string() })?;
            mouse_move(mx, my, relative.unwrap_or(false)).await
        }
        "mouse_click" => mouse_click(button.unwrap_or("left"), count).await,
        "mouse_drag" => {
            let sx = x.ok_or_else(|| ToolError::InvalidArguments { message: "x (start_x) required".to_string() })?;
            let sy = y.ok_or_else(|| ToolError::InvalidArguments { message: "y (start_y) required".to_string() })?;
            let ex = x2.ok_or_else(|| ToolError::InvalidArguments { message: "x2 (end_x) required".to_string() })?;
            let ey = y2.ok_or_else(|| ToolError::InvalidArguments { message: "y2 (end_y) required".to_string() })?;
            mouse_drag(sx, sy, ex, ey, button.unwrap_or("left")).await
        }
        "mouse_scroll" => {
            let amt = amount.ok_or_else(|| ToolError::InvalidArguments { message: "amount required".to_string() })?;
            let dir = direction.unwrap_or("down");
            mouse_scroll(amt, dir).await
        }
        "screenshot" => screenshot(scale_percent).await,
        "capture_window" => {
            let title = window_title.ok_or_else(|| ToolError::InvalidArguments { message: "window_title required".to_string() })?;
            capture_window(title, scale_percent).await
        }
        "clipboard_read" => clipboard_read().await,
        "clipboard_write" => {
            let txt = text.ok_or_else(|| ToolError::InvalidArguments { message: "text required".to_string() })?;
            clipboard_write(txt).await
        }
        _ => Err(ToolError::InvalidArguments { message: format!("Unknown app action: {}", action) }),
    }
}
