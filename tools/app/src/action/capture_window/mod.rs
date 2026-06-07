use ene_tool_common::prelude::*;
use image::{DynamicImage, imageops::FilterType};

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
        let title = self.window_title.clone();

        let image = if crate::utils::portal::detect_wayland() {
            portal::capture_window_portal(scale_percent).await
        } else {
            let t = title.clone();
            let sp = scale_percent;
            tokio::task::spawn_blocking(move || capture_window_by_title_xcap(&t, sp))
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Task failed: {e}"),
                })?
        }?;

        let data_uri = crate::utils::encode_image_to_data_uri(image)?;
        let result = serde_json::json!({
            "type": "screenshot",
            "data": data_uri,
            "window": title
        });
        Ok(result.to_string())
    }
}
