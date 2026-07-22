use ene_tool_common::prelude::*;
use image::DynamicImage;

#[cfg(target_os = "linux")]
mod portal;

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
        if (win_title.contains(title) || app_name.contains(title))
            && !is_minimized
            && let Ok(img) = window.capture_image()
        {
            let image = DynamicImage::ImageRgba8(img);
            return Ok(crate::utils::image::resize_image(image, scale_percent));
        }
    }

    Err(ToolError::ExecutionFailed {
        message: format!("Window not found: {title}"),
    })
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "app",
    name = "capture_window",
    summary = "Takes a screenshot of a specific window by title substring match.",
    description = "Takes a screenshot of a specific window by title substring match.",
    category = "App",
    keywords_primary = "window, screenshot, capture"
)]
pub struct CaptureWindowAction {
    /// Substring of window title or app name to capture.
    window_title: String,
    /// Resize percentage 1-100 (default: 50).
    #[arg(minimum = 1, maximum = 100, default = "50")]
    #[serde(default)]
    scale_percent: Option<u32>,
}

impl CaptureWindowAction {
    async fn run(&self) -> Result<String, ToolError> {
        let scale_percent = self
            .scale_percent
            .unwrap_or(crate::config::DEFAULT_SCALE_PERCENT);

        let image = if crate::utils::portal::detect_wayland() {
            #[cfg(target_os = "linux")]
            {
                portal::capture_window_portal(scale_percent).await
            }
            #[cfg(not(target_os = "linux"))]
            {
                let t = self.window_title.clone();
                let sp = scale_percent;
                tokio::task::spawn_blocking(move || capture_window_by_title_xcap(&t, sp))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed {
                        message: format!("Task failed: {e}"),
                    })?
            }
        } else {
            let t = self.window_title.clone();
            let sp = scale_percent;
            tokio::task::spawn_blocking(move || capture_window_by_title_xcap(&t, sp))
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Task failed: {e}"),
                })?
        }?;

        let data_uri = crate::utils::encode_image_to_data_uri(&image)?;
        let result = serde_json::json!({
            "type": "screenshot",
            "data": data_uri,
            "window": self.window_title
        });
        Ok(result.to_string())
    }
}
