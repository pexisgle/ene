use super::definition::{ToolDefinition, ToolRegistry};
use async_trait::async_trait;
use image::{DynamicImage, imageops::FilterType};
use std::io::Cursor;
use base64::{Engine as _, engine::general_purpose};

pub struct ScreenshotToolRegistry {
    scale_percent: u32,
}

impl ScreenshotToolRegistry {
    pub fn new(scale_percent: u32) -> Self {
        Self { scale_percent }
    }

    fn capture_screen(&self) -> Result<DynamicImage, String> {
        let mut target_image = None;

        // 1. Try to get active window
        if let Ok(active_win) = active_win_pos_rs::get_active_window() {
            if let Ok(windows) = xcap::Window::all() {
                for window in windows {
                    // Title or App name matching
                    // Wayland/X11 edge cases: xcap might have slightly different titles than active-win-pos-rs
                    // We check if title matches or app name matches, or ID if possible.
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

        // 2. Fallback to primary monitor (or first monitor)
        if target_image.is_none() {
            if let Ok(monitors) = xcap::Monitor::all() {
                let primary = monitors.iter().find(|m| m.is_primary().unwrap_or(false)).unwrap_or_else(|| &monitors[0]);
                if let Ok(img) = primary.capture_image() {
                    target_image = Some(DynamicImage::ImageRgba8(img));
                }
            }
        }

        let image = target_image.ok_or_else(|| "Failed to capture any window or monitor".to_string())?;

        // 3. Resize if needed
        let final_image = if self.scale_percent > 0 && self.scale_percent < 100 {
            let nwidth = (image.width() as f32 * (self.scale_percent as f32 / 100.0)) as u32;
            let nheight = (image.height() as f32 * (self.scale_percent as f32 / 100.0)) as u32;
            image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
        } else {
            image
        };

        Ok(final_image)
    }
}

#[async_trait]
impl ToolRegistry for ScreenshotToolRegistry {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "take_screenshot".to_string(),
                description: "Takes a screenshot of the user's active window or primary monitor and returns the image data for analysis. Use this when you need to see what the user is currently looking at on their screen.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            }
        ]
    }

    async fn call_tool(&self, name: &str, _arguments: &str) -> Result<String, String> {
        if name != "take_screenshot" {
            return Err(format!("Unknown tool: {}", name));
        }

        let image = self.capture_screen()?;

        let mut buf = Cursor::new(Vec::new());
        image.write_to(&mut buf, image::ImageFormat::Png)
            .map_err(|e| format!("Failed to encode image to PNG: {}", e))?;

        let b64 = general_purpose::STANDARD.encode(buf.into_inner());
        let data_uri = format!("data:image/png;base64,{}", b64);

        // stream.rs will intercept this specific JSON format
        let result = serde_json::json!({
            "type": "screenshot",
            "data": data_uri
        });

        Ok(result.to_string())
    }
}
