use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolExample, ToolName, ToolSpec, ToolVersion,
};
use ene_tools_common::ToolAction;
use image::{DynamicImage, imageops::FilterType};
use serde::Deserialize;

mod portal;

#[derive(Deserialize)]
struct ScreenshotArgs {
    #[serde(default)]
    scale_percent: Option<u32>,
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

/// Action to take a screenshot.
pub struct ScreenshotAction;

#[async_trait]
impl ToolAction for ScreenshotAction {
    fn tool_name(&self) -> &'static str {
        "app.screenshot"
    }

    fn definition(&self) -> ToolSpec {
        ToolSpec {
            name: ToolName::new("app.screenshot"),
            version: ToolVersion::default(),
            display_name: "Takes a screenshot of the active window or primary screen.".to_string(),
            summary: "Takes a screenshot of the active window or primary screen.".to_string(),
            description: "Takes a screenshot of the active window or primary screen.".to_string(),
            category: ToolCategory::App,
            keywords: KeywordSet::primary_only(["screenshot", "screen", "capture", "image"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "scale_percent": { "type": "integer", "description": "Resize percentage 1-100 (default: 50)" }
                },
                "required": []
            }),
            examples: vec![
                ToolExample {
                    description: "Take a full screenshot".to_string(),
                    input: serde_json::json!({}),
                    output: None,
                },
                ToolExample {
                    description: "Take screenshot with custom scale".to_string(),
                    input: serde_json::json!({"scale_percent": 25}),
                    output: None,
                },
            ],
            caveats: Vec::new(),
            side_effects: SideEffects::default(),
            preconditions: Vec::new(),
            related: Vec::new(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: ScreenshotArgs = serde_json::from_str(arguments).unwrap_or(ScreenshotArgs {
            scale_percent: None,
        });

        let scale_percent = args
            .scale_percent
            .unwrap_or(crate::config::DEFAULT_SCALE_PERCENT);

        let image = if crate::utils::portal::detect_wayland() {
            portal::capture_screen_portal(scale_percent).await
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
