use ene_plugin::prelude::*;
use image::DynamicImage;

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
            {
                match window.capture_image() {
                    Ok(img) => {
                        target_image = Some(DynamicImage::ImageRgba8(img));
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to capture active window: {e}");
                    }
                }
            }
        }
    }

    if target_image.is_none()
        && let Ok(monitors) = xcap::Monitor::all()
    {
        if monitors.is_empty() {
            return Err(ToolError::execution_failed("No monitors found".to_string()));
        }
        let primary = monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .unwrap_or(&monitors[0]);
        if let Ok(img) = primary.capture_image() {
            target_image = Some(DynamicImage::ImageRgba8(img));
        }
    }

    let image = target_image
        .ok_or_else(|| ToolError::execution_failed("Failed to capture screen".to_string()))?;

    Ok(crate::utils::image::resize_image(image, scale_percent))
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
                    .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
            }
        } else {
            let sp = scale_percent;
            tokio::task::spawn_blocking(move || capture_screen_xcap(sp))
                .await
                .map_err(|e| ToolError::execution_failed(format!("Task failed: {e}")))?
        }?;

        let data_uri = crate::utils::encode_image_to_data_uri(&image)?;
        let result = serde_json::json!({
            "type": "screenshot",
            "data": data_uri
        });
        Ok(result.to_string())
    }
}
