//! Active-window / primary-display capture for proactive screen summary.
//!
//! Images stay in desktop memory only. Callers must drop them after
//! producing a text summary — never forward bytes into mind / store.

use image::{DynamicImage, imageops::FilterType};

/// Capture scale for vision: 50% balances legibility vs token/cost budget.
/// (35% was too aggressive — local vision misread text editors as media UIs.)
const DEFAULT_SCALE_PERCENT: u32 = 50;

/// Capture result with optional active-app label for cache keys.
#[derive(Debug)]
pub struct CapturedScreen {
    /// Downscaled RGBA/RGB image.
    pub image: DynamicImage,
    /// Privacy-safe active application label (may be empty).
    pub app_label: String,
}

/// Capture the active window when possible, otherwise the primary display.
/// When Ene is focused, capture the primary display so background context remains visible.
pub async fn capture_for_summary() -> Result<CapturedScreen, String> {
    let app_label = active_app_label().unwrap_or_default();
    let ene_focused = is_self_app(&app_label);

    let image = if detect_wayland() {
        capture_wayland(DEFAULT_SCALE_PERCENT, ene_focused).await?
    } else {
        let scale = DEFAULT_SCALE_PERCENT;
        tokio::task::spawn_blocking(move || capture_xcap(scale, ene_focused))
            .await
            .map_err(|e| format!("capture task failed: {e}"))??
    };

    Ok(CapturedScreen { image, app_label })
}

fn capture_xcap(scale_percent: u32, force_primary_display: bool) -> Result<DynamicImage, String> {
    let mut target_image = None;

    if !force_primary_display
        && let Ok(active_win) = active_win_pos_rs::get_active_window()
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

    let image = target_image.ok_or_else(|| "failed to capture screen".to_string())?;
    Ok(resize_image(image, scale_percent))
}

#[cfg(target_os = "linux")]
async fn capture_wayland(
    scale_percent: u32,
    force_primary_display: bool,
) -> Result<DynamicImage, String> {
    use ashpd::desktop::screenshot::AvailableTargets;

    let target = if force_primary_display {
        AvailableTargets::Screen
    } else {
        AvailableTargets::ActiveWindow
    };

    match capture_wayland_portal(scale_percent, target).await {
        Ok(image) => Ok(image),
        Err(portal_err) if force_primary_display => {
            tracing::debug!(
                component = "ProactiveObserve",
                error = %portal_err,
                "Portal screen capture failed; falling back to monitor capture"
            );
            let scale = scale_percent;
            tokio::task::spawn_blocking(move || capture_xcap(scale, true))
                .await
                .map_err(|e| format!("capture task failed: {e}"))?
        }
        Err(portal_err) => Err(portal_err),
    }
}

#[cfg(target_os = "linux")]
async fn capture_wayland_portal(
    scale_percent: u32,
    target: ashpd::desktop::screenshot::AvailableTargets,
) -> Result<DynamicImage, String> {
    use ashpd::desktop::screenshot::Screenshot;

    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .target(target)
        .send()
        .await
        .map_err(|e| format!("portal screenshot request failed: {e}"))?
        .response()
        .map_err(|e| format!("portal screenshot response failed: {e}"))?;

    let uri = response.uri();
    let path = uri.as_str().strip_prefix("file://").unwrap_or(uri.as_str());
    let image = image::open(path).map_err(|e| format!("failed to open screenshot file: {e}"))?;
    drop(std::fs::remove_file(path));
    Ok(resize_image(image, scale_percent))
}

#[cfg(not(target_os = "linux"))]
async fn capture_wayland(
    scale_percent: u32,
    force_primary_display: bool,
) -> Result<DynamicImage, String> {
    let scale = scale_percent;
    tokio::task::spawn_blocking(move || capture_xcap(scale, force_primary_display))
        .await
        .map_err(|e| format!("capture task failed: {e}"))?
}

fn resize_image(image: DynamicImage, scale_percent: u32) -> DynamicImage {
    if scale_percent == 0 || scale_percent >= 100 {
        return image;
    }
    let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
    let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
    image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
}

fn active_app_label() -> Option<String> {
    match active_win_pos_rs::get_active_window() {
        Ok(win) => {
            let app = win.app_name.trim();
            if app.is_empty() {
                None
            } else {
                Some(super::redact_paths(app))
            }
        }
        Err(()) => None,
    }
}

fn is_self_app(label: &str) -> bool {
    let lower = label.to_ascii_lowercase();
    lower.contains("ene-desktop") || lower == "ene"
}

fn detect_wayland() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s == "wayland")
            || std::env::var("WAYLAND_DISPLAY").is_ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_app_detection() {
        assert!(is_self_app("ene-desktop"));
        assert!(is_self_app("Ene"));
        assert!(!is_self_app("firefox"));
    }

    #[test]
    fn resize_noop_at_100() {
        let img = DynamicImage::new_rgba8(10, 10);
        let out = resize_image(img, 100);
        assert_eq!(out.width(), 10);
    }
}
