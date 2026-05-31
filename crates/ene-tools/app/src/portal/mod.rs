#[cfg(target_os = "linux")]
mod capture;
mod compositor;

use image::DynamicImage;

#[cfg(target_os = "linux")]
use image::imageops::FilterType;

#[cfg(not(target_os = "linux"))]
use ene_tool_proto::ToolError;

#[cfg(target_os = "linux")]
pub fn detect_wayland() -> bool {
    std::env::var("XDG_SESSION_TYPE")
        .map(|s| s == "wayland")
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY").is_ok()
}

#[cfg(not(target_os = "linux"))]
pub fn detect_wayland() -> bool {
    false
}

#[cfg(target_os = "linux")]
pub(crate) fn resize_image(image: DynamicImage, scale_percent: u32) -> DynamicImage {
    if scale_percent > 0 && scale_percent < 100 {
        let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
        let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
        image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
    } else {
        image
    }
}

// Re-export capture functions
#[cfg(target_os = "linux")]
pub use capture::{capture_screen_portal, capture_window_portal};

#[cfg(not(target_os = "linux"))]
pub async fn capture_screen_portal(_scale_percent: u32) -> Result<DynamicImage, ToolError> {
    Err(ToolError::ExecutionFailed {
        message: "Portal not available on this platform".to_string(),
    })
}

#[cfg(not(target_os = "linux"))]
pub async fn capture_window_portal(_scale_percent: u32) -> Result<DynamicImage, ToolError> {
    Err(ToolError::ExecutionFailed {
        message: "Portal not available on this platform".to_string(),
    })
}

// Re-export compositor dispatch functions
pub use compositor::{focus_window_wayland, list_windows_wayland};
