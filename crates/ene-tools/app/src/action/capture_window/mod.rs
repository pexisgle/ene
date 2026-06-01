use async_trait::async_trait;
use ene_tool_proto::{ToolDefinition, ToolError};
use ene_tools_common::ToolAction;
use image::{DynamicImage, imageops::FilterType};
use serde::Deserialize;

mod portal;

#[derive(Deserialize)]
struct CaptureWindowArgs {
    window_title: String,
    #[serde(default)]
    scale_percent: Option<u32>,
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

/// Action to capture a specific window.
pub struct CaptureWindowAction;

#[async_trait]
impl ToolAction for CaptureWindowAction {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "capture_window".to_string(),
            description: "Takes a screenshot of a specific window by title.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "window_title": { "type": "string", "description": "Window title" },
                    "scale_percent": { "type": "integer", "description": "Resize percentage" }
                },
                "required": ["window_title"]
            }),
            category: Some(ene_tool_proto::ToolCategory::App),
            keywords: vec![
                "window".to_string(),
                "screenshot".to_string(),
                "capture".to_string(),
            ],
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: CaptureWindowArgs =
            serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
                message: format!("Invalid arguments: {e}"),
            })?;

        let scale_percent = args
            .scale_percent
            .unwrap_or(crate::config::DEFAULT_SCALE_PERCENT);
        let title = args.window_title.clone();

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
