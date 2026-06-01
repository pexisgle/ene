use ene_tool_proto::ToolError;
use enigo::{Key, Keyboard};

pub(crate) fn parse_key(name: &str) -> Option<Key> {
    match name.to_lowercase().as_str() {
        "ctrl" | "control" => Some(Key::Control),
        "shift" => Some(Key::Shift),
        "alt" | "option" => Some(Key::Alt),
        "meta" | "super" | "cmd" | "command" | "win" | "windows" => Some(Key::Meta),
        "return" | "enter" => Some(Key::Return),
        "escape" | "esc" => Some(Key::Escape),
        "tab" => Some(Key::Tab),
        "space" => Some(Key::Space),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "page_up" => Some(Key::PageUp),
        "pagedown" | "page_down" => Some(Key::PageDown),
        "up" | "arrow_up" => Some(Key::UpArrow),
        "down" | "arrow_down" => Some(Key::DownArrow),
        "left" | "arrow_left" => Some(Key::LeftArrow),
        "right" | "arrow_right" => Some(Key::RightArrow),
        "insert" | "ins" => Some(Key::Insert),
        "capslock" | "caps_lock" => Some(Key::CapsLock),
        "f1" => Some(Key::F1),
        "f2" => Some(Key::F2),
        "f3" => Some(Key::F3),
        "f4" => Some(Key::F4),
        "f5" => Some(Key::F5),
        "f6" => Some(Key::F6),
        "f7" => Some(Key::F7),
        "f8" => Some(Key::F8),
        "f9" => Some(Key::F9),
        "f10" => Some(Key::F10),
        "f11" => Some(Key::F11),
        "f12" => Some(Key::F12),
        s if s.len() == 1 => Some(Key::Unicode(s.chars().next().unwrap())),
        _ => None,
    }
}

pub async fn type_text(text: &str) -> Result<String, ToolError> {
    let text = text.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        enigo.text(&text).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Type failed: {e}"),
        })?;
        Ok::<_, ToolError>("Text typed successfully.".to_string())
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

pub async fn press_key(key: &str) -> Result<String, ToolError> {
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;

        if let Some(enigo_key) = parse_key(&key) {
            enigo.key(enigo_key, enigo::Direction::Click).map_err(|e| {
                ToolError::ExecutionFailed {
                    message: format!("Key press failed: {e}"),
                }
            })?;
            Ok::<_, ToolError>(format!("Pressed key: {}", key))
        } else {
            Err(ToolError::ExecutionFailed {
                message: format!("Unsupported key: {}", key),
            })
        }
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

pub async fn key_combo(combo: &str) -> Result<String, ToolError> {
    let combo = combo.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;

        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
        if parts.is_empty() {
            return Err(ToolError::ExecutionFailed {
                message: "Empty key combo".to_string(),
            });
        }

        let keys: Vec<Key> = parts.iter().filter_map(|p| parse_key(p)).collect();

        if keys.len() != parts.len() {
            let unrecognized: Vec<&&str> = parts
                .iter()
                .enumerate()
                .filter(|(i, _)| parse_key(parts[*i]).is_none())
                .map(|(_, p)| p)
                .collect();
            return Err(ToolError::ExecutionFailed {
                message: format!("Unrecognized key(s) in combo: {:?}", unrecognized),
            });
        }

        let modifier_count = keys.len().saturating_sub(1);
        for key in &keys[..modifier_count] {
            enigo
                .key(*key, enigo::Direction::Press)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Key press failed: {e}"),
                })?;
        }

        if let Some(last) = keys.last() {
            enigo
                .key(*last, enigo::Direction::Click)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Key click failed: {e}"),
                })?;
        }

        for key in keys[..modifier_count].iter().rev() {
            enigo
                .key(*key, enigo::Direction::Release)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Key release failed: {e}"),
                })?;
        }

        Ok::<_, ToolError>(format!("Executed key combo: {}", combo))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}
