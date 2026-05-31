use ene_tool_proto::ToolError;

pub async fn list_windows() -> Result<String, ToolError> {
    if crate::portal::detect_wayland() {
        return crate::portal::list_windows_wayland();
    }

    let windows = xcap::Window::all().map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to enumerate windows: {e}"),
    })?;

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
    if crate::portal::detect_wayland() {
        return crate::portal::focus_window_wayland(title);
    }

    let windows = xcap::Window::all().map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to enumerate windows: {e}"),
    })?;

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
    Err(ToolError::ExecutionFailed {
        message: format!("Window not found: {}", title),
    })
}

pub async fn get_active_window() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let active_win =
            active_win_pos_rs::get_active_window().map_err(|_| ToolError::ExecutionFailed {
                message: "Failed to get active window".to_string(),
            })?;
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
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

pub async fn list_monitors() -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let monitors = xcap::Monitor::all().map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to enumerate monitors: {e}"),
        })?;

        let mut result = Vec::new();
        for monitor in monitors {
            let name = monitor.name().unwrap_or_else(|_| "Unknown".to_string());
            let id = monitor.id();
            let id_str = match id {
                Ok(v) => v.to_string(),
                Err(_) => "?".to_string(),
            };
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
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}
