use ene_tool_proto::ToolError;
use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose};
use image::{DynamicImage, imageops::FilterType};

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
            let primary = monitors
                .iter()
                .find(|m| m.is_primary().unwrap_or(false))
                .unwrap_or_else(|| &monitors[0]);
            if let Ok(img) = primary.capture_image() {
                target_image = Some(DynamicImage::ImageRgba8(img));
            }
        }
    }

    let image = target_image.ok_or_else(|| ToolError::ExecutionFailed {
        message: "Failed to capture screen".to_string(),
    })?;

    let final_image = if scale_percent > 0 && scale_percent < 100 {
        let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
        let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
        image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
    } else {
        image
    };

    Ok(final_image)
}

fn capture_window_by_title_xcap(
    title: &str,
    scale_percent: u32,
) -> Result<DynamicImage, ToolError> {
    let windows = xcap::Window::all().map_err(|e| ToolError::ExecutionFailed {
        message: format!("Failed to enumerate windows: {e}"),
    })?;

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

    Err(ToolError::ExecutionFailed {
        message: format!("Window not found: {}", title),
    })
}

pub(crate) fn encode_image_to_data_uri(image: DynamicImage) -> Result<String, ToolError> {
    let mut buf = Cursor::new(Vec::new());
    image
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to encode image to PNG: {}", e),
        })?;
    let b64 = general_purpose::STANDARD.encode(buf.into_inner());
    Ok(format!("data:image/png;base64,{}", b64))
}

pub async fn screenshot(scale_percent: Option<u32>) -> Result<String, ToolError> {
    let scale_percent = scale_percent.unwrap_or(50);

    let image = if crate::portal::detect_wayland() {
        crate::portal::capture_screen_portal(scale_percent).await
    } else {
        let sp = scale_percent;
        tokio::task::spawn_blocking(move || capture_screen_xcap(sp))
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Task failed: {e}"),
            })?
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

    let image = if crate::portal::detect_wayland() {
        crate::portal::capture_window_portal(scale_percent).await
    } else {
        let t = title.to_string();
        let sp = scale_percent;
        tokio::task::spawn_blocking(move || capture_window_by_title_xcap(&t, sp))
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Task failed: {e}"),
            })?
    }?;

    let data_uri = encode_image_to_data_uri(image)?;
    let result = serde_json::json!({
        "type": "screenshot",
        "data": data_uri,
        "window": title
    });
    Ok(result.to_string())
}
