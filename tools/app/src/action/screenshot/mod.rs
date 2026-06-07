use ene_tool_common::prelude::*;
use image::{DynamicImage, imageops::FilterType};

#[cfg(target_os = "linux")]
mod portal;

fn capture_screen_xcap(scale_percent: u32) -> Result<DynamicImage, ToolError> {
    let mut target_image = None;

    if let Ok(active_win) = active_win_pos_rs::get_active_window()
        && let Ok(windows) = xcap::Window::all()
    {
        for window in windows {
            let title = window.title().unwrap_or_default();
            let app_name = window.app_name().unwrap_or_default();
            if (title == active_win.title || app_name == active_win.app_name)
                && !window.is_minimized().unwrap_or(false)
                && let Ok(img) = window.capture_image()
            {
                target_image = Some(DynamicImage::ImageRgba8(img));
                break;
            }
        }
    }

    if target_image.is_none()
        && let Ok(monitors) = xcap::Monitor::all()
    {
        let primary = monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .unwrap_or_else(|| &monitors[0]);
        if let Ok(img) = primary.capture_image() {
            target_image = Some(DynamicImage::ImageRgba8(img));
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

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "screenshot",
    summary = "Takes a screenshot of the active window or primary screen.",
    description = "Takes a screenshot of the active window or primary screen.",
    category = "App",
    keywords_primary = "screenshot, screen, capture, image"
)]
pub struct ScreenshotAction {
    /// Resize percentage 1-100 (default: 50).
    #[arg(minimum = 1, maximum = 100, default = "50")]
    #[serde(default)]
    scale_percent: Option<u32>,
}

impl ScreenshotAction {
    async fn run(&self) -> Result<String, ToolError> {
        let scale_percent = self
            .scale_percent
            .unwrap_or(crate::config::DEFAULT_SCALE_PERCENT);

        let image = if crate::utils::portal::detect_wayland() {
            #[cfg(target_os = "linux")]
            {
                portal::capture_screen_portal(scale_percent).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let sp = scale_percent;
                tokio::task::spawn_blocking(move || capture_screen_xcap(sp))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Task failed: {e}"),
                    })?
            }
        } else {
            let sp = scale_percent;
            tokio::task::spawn_blocking(move || capture_screen_xcap(sp))
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Task failed: {e}"),
                })?
        }?;

        let data_uri = crate::utils::encode_image_to_data_uri(image)?;
        let result = serde_json::json!({
            "type": "screenshot",
            "data": data_uri
        });
        Ok(result.to_string())
    }
}
