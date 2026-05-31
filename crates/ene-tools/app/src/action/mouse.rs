use ene_tool_proto::ToolError;
use enigo::{Axis, Coordinate, Direction, Mouse};

pub async fn mouse_move(x: i32, y: i32, relative: bool) -> Result<String, ToolError> {
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        let coord = if relative {
            Coordinate::Rel
        } else {
            Coordinate::Abs
        };
        enigo
            .move_mouse(x, y, coord)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;
        let mode = if relative { "relative" } else { "absolute" };
        Ok::<_, ToolError>(format!("Mouse moved to ({}, {}) [{}]", x, y, mode))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

pub async fn mouse_click(button: &str, count: Option<u32>) -> Result<String, ToolError> {
    let button = button.to_string();
    let count = count.unwrap_or(1);
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        let btn = match button.as_str() {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            "back" => enigo::Button::Back,
            "forward" => enigo::Button::Forward,
            _ => enigo::Button::Left,
        };
        for _ in 0..count {
            enigo
                .button(btn, Direction::Click)
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Mouse click failed: {e}"),
                })?;
        }
        Ok::<_, ToolError>(format!("Mouse {} click x{}", button, count))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}

pub async fn mouse_drag(
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    button: &str,
) -> Result<String, ToolError> {
    let button = button.to_string();
    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;

        enigo
            .move_mouse(start_x, start_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        let btn = match button.as_str() {
            "right" => enigo::Button::Right,
            "middle" => enigo::Button::Middle,
            _ => enigo::Button::Left,
        };
        enigo
            .button(btn, Direction::Press)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse press failed: {e}"),
            })?;

        enigo
            .move_mouse(end_x, end_y, Coordinate::Abs)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse move failed: {e}"),
            })?;

        enigo
            .button(btn, Direction::Release)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Mouse release failed: {e}"),
            })?;

        Ok::<_, ToolError>(format!(
            "Dragged from ({},{}) to ({},{}) with {} button",
            start_x, start_y, end_x, end_y, button
        ))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
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
    let axis = if is_horizontal {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };

    tokio::task::spawn_blocking(move || {
        let mut enigo = enigo::Enigo::new(&enigo::Settings::default()).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Failed to initialize enigo: {e}"),
            }
        })?;
        enigo
            .scroll(scroll_amount, axis)
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Scroll failed: {e}"),
            })?;
        Ok::<_, ToolError>(format!("Scrolled {} by {}", dir_str, amount))
    })
    .await
    .map_err(|e| ToolError::ExecutionFailed {
        message: format!("Task failed: {e}"),
    })?
}
