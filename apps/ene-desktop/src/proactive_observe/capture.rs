//! Active-window / primary-display capture for proactive screen summary.
//!
//! Images stay in desktop memory only. Callers must drop them after
//! producing a text summary — never forward bytes into mind / store.

use image::{DynamicImage, GenericImageView, RgbaImage, imageops::FilterType};

use super::roi::crop_roi;

/// Capture scale for vision: 50% balances legibility vs token/cost budget.
/// (35% was too aggressive — local vision misread text editors as media UIs.)
const DEFAULT_SCALE_PERCENT: u32 = 50;

/// Pixel budget for the composited frame. Must stay at or below the runtime
/// validation limit (`ene_runtime::vision::MAX_PIXELS`) or the vision call
/// rejects the frame.
const MAX_COMPOSITE_PIXELS: u64 = 1920 * 1080;

const _: () = assert!(MAX_COMPOSITE_PIXELS <= ene_runtime::vision::MAX_PIXELS);

/// Width of the dark separator between overview and ROI in the composite.
const SEPARATOR_PX: u32 = 8;

/// Capture result with optional active-app label for cache keys.
#[derive(Debug)]
pub struct CapturedScreen {
    /// Downscaled RGBA/RGB image (the overview).
    pub image: DynamicImage,
    /// 100%-scale crop around the cursor, when the cursor position and the
    /// captured surface's global geometry are known.
    pub roi: Option<DynamicImage>,
    /// Privacy-safe active application label (may be empty).
    pub app_label: String,
    /// True when the active window title/class looks like a code editor or
    /// terminal; the raw title never leaves this function.
    pub is_code_like: bool,
}

/// Capture the active window when possible, otherwise the primary display.
/// When Ene is focused, capture the primary display so background context
/// remains visible. `cursor` is the global pointer position used to crop the
/// 100%-scale ROI from the full-resolution capture.
pub async fn capture_for_summary(cursor: Option<(i32, i32)>) -> Result<CapturedScreen, String> {
    let (app_label, is_code_like) = active_window_signals();
    let ene_focused = is_self_app(&app_label);

    let (image, roi) = if detect_wayland() {
        capture_wayland(DEFAULT_SCALE_PERCENT, ene_focused, cursor).await?
    } else {
        let scale = DEFAULT_SCALE_PERCENT;
        tokio::task::spawn_blocking(move || capture_xcap(scale, ene_focused, cursor))
            .await
            .map_err(|e| format!("capture task failed: {e}"))??
    };

    Ok(CapturedScreen {
        image,
        roi,
        app_label,
        is_code_like,
    })
}

/// Full-resolution capture plus the global origin of the captured surface,
/// so the ROI can translate the global cursor into surface-local pixels.
type CapturedSurface = (DynamicImage, Option<(i32, i32)>);

fn capture_xcap(
    scale_percent: u32,
    force_primary_display: bool,
    cursor: Option<(i32, i32)>,
) -> Result<(DynamicImage, Option<DynamicImage>), String> {
    let mut target: Option<CapturedSurface> = None;

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
                let origin = window.x().ok().zip(window.y().ok());
                target = Some((DynamicImage::ImageRgba8(img), origin));
                break;
            }
        }
    }

    if target.is_none()
        && let Ok(monitors) = xcap::Monitor::all()
    {
        let monitor = monitors
            .iter()
            .find(|m| m.is_primary().unwrap_or(false))
            .or_else(|| monitors.first());
        if let Some(monitor) = monitor
            && let Ok(img) = monitor.capture_image()
        {
            let origin = monitor.x().ok().zip(monitor.y().ok());
            target = Some((DynamicImage::ImageRgba8(img), origin));
        }
    }

    let (full, origin) = target.ok_or_else(|| "failed to capture screen".to_string())?;
    let roi = cursor.and_then(|c| origin.and_then(|o| crop_roi(&full, c, o)));
    Ok((resize_image(full, scale_percent), roi))
}

#[cfg(target_os = "linux")]
async fn capture_wayland(
    scale_percent: u32,
    force_primary_display: bool,
    cursor: Option<(i32, i32)>,
) -> Result<(DynamicImage, Option<DynamicImage>), String> {
    use ashpd::desktop::screenshot::AvailableTargets;

    let target = if force_primary_display {
        AvailableTargets::Screen
    } else {
        AvailableTargets::ActiveWindow
    };

    match capture_wayland_portal(scale_percent, target, cursor).await {
        Ok(frame) => Ok(frame),
        Err(portal_err) if force_primary_display => {
            tracing::debug!(
                component = "ProactiveObserve",
                error = %portal_err,
                "Portal screen capture failed; falling back to monitor capture"
            );
            let scale = scale_percent;
            tokio::task::spawn_blocking(move || capture_xcap(scale, true, cursor))
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
    cursor: Option<(i32, i32)>,
) -> Result<(DynamicImage, Option<DynamicImage>), String> {
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

    // The portal gives no geometry; only compositors whose active-window
    // position `active_win_pos_rs` can report (KWin/Hyprland) allow mapping
    // the global cursor into the captured window. This is best-effort: the
    // pointer still comes from device_query over XWayland, which freezes over
    // native Wayland surfaces, and HiDPI mixes logical geometry with physical
    // pixels. Screen captures are assumed to start at the global origin; a
    // wrong guess only misplaces the optional ROI.
    let origin = match target {
        ashpd::desktop::screenshot::AvailableTargets::ActiveWindow => {
            active_win_pos_rs::get_position()
                .ok()
                .map(|p| (p.x as i32, p.y as i32))
        }
        ashpd::desktop::screenshot::AvailableTargets::Screen => Some((0, 0)),
        // Window/Area targets are never requested here; no geometry to map.
        _ => None,
    };
    let roi = cursor.and_then(|c| origin.and_then(|o| crop_roi(&image, c, o)));
    Ok((resize_image(image, scale_percent), roi))
}

#[cfg(not(target_os = "linux"))]
async fn capture_wayland(
    scale_percent: u32,
    force_primary_display: bool,
    cursor: Option<(i32, i32)>,
) -> Result<(DynamicImage, Option<DynamicImage>), String> {
    let scale = scale_percent;
    tokio::task::spawn_blocking(move || capture_xcap(scale, force_primary_display, cursor))
        .await
        .map_err(|e| format!("capture task failed: {e}"))?
}

/// The composite handed to the vision model: the downscaled overview with the
/// 100%-scale ROI beside it, separated by a dark bar. The overview is shrunk
/// when needed so the frame stays within [`MAX_COMPOSITE_PIXELS`]; the ROI
/// always keeps its native scale. Returns the composite and the overview as
/// placed, the latter being what the diff gate fingerprints.
pub fn compose(
    overview: &DynamicImage,
    roi: Option<&DynamicImage>,
) -> (DynamicImage, DynamicImage) {
    let Some(roi) = roi else {
        let overview = fit_to_budget(overview.clone());
        return (overview.clone(), overview);
    };

    let mut w = overview.width();
    let mut h = overview.height();
    for _ in 0..8 {
        let canvas_w = w + SEPARATOR_PX + roi.width();
        let canvas_h = h.max(roi.height());
        if u64::from(canvas_w) * u64::from(canvas_h) <= MAX_COMPOSITE_PIXELS {
            break;
        }
        // Only the overview may give up pixels; the ROI keeps 100% scale.
        let available_w = MAX_COMPOSITE_PIXELS as f32 / canvas_h as f32;
        let target_w = available_w - SEPARATOR_PX as f32 - roi.width() as f32;
        if target_w >= w as f32 {
            break;
        }
        let scale = target_w / w as f32;
        w = ((w as f32 * scale).round() as u32).max(1);
        h = ((h as f32 * scale).round() as u32).max(1);
    }
    let overview = resize_to(overview.clone(), w, h);

    let canvas_w = w + SEPARATOR_PX + roi.width();
    let canvas_h = h.max(roi.height());
    let mut canvas = RgbaImage::from_pixel(canvas_w, canvas_h, image::Rgba([16, 16, 16, 255]));
    image::imageops::overlay(&mut canvas, &overview, 0, 0);
    image::imageops::overlay(&mut canvas, roi, i64::from(w + SEPARATOR_PX), 0);
    (DynamicImage::ImageRgba8(canvas), overview)
}

/// Scale an image down to the pixel budget when it exceeds it (very large
/// displays captured without a usable ROI).
fn fit_to_budget(image: DynamicImage) -> DynamicImage {
    let (w, h) = image.dimensions();
    if u64::from(w) * u64::from(h) <= MAX_COMPOSITE_PIXELS {
        return image;
    }
    let scale = (MAX_COMPOSITE_PIXELS as f32 / (u64::from(w) * u64::from(h)) as f32).sqrt();
    resize_to(
        image,
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

fn resize_to(image: DynamicImage, width: u32, height: u32) -> DynamicImage {
    image.resize(width.max(1), height.max(1), FilterType::Lanczos3)
}

fn resize_image(image: DynamicImage, scale_percent: u32) -> DynamicImage {
    if scale_percent == 0 || scale_percent >= 100 {
        return image;
    }
    let nwidth = (image.width() as f32 * (scale_percent as f32 / 100.0)) as u32;
    let nheight = (image.height() as f32 * (scale_percent as f32 / 100.0)) as u32;
    image.resize(nwidth.max(1), nheight.max(1), FilterType::Lanczos3)
}

fn active_window_signals() -> (String, bool) {
    match active_win_pos_rs::get_active_window() {
        Ok(win) => {
            let app = win.app_name.trim();
            let label = if app.is_empty() {
                String::new()
            } else {
                super::redact_paths(app)
            };
            let is_code_like = super::ocr::is_code_window(&win.title, &win.app_name);
            (label, is_code_like)
        }
        Err(()) => (String::new(), false),
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

    #[test]
    fn compose_places_roi_next_to_overview() {
        let overview = DynamicImage::new_rgba8(960, 540);
        let roi = DynamicImage::new_rgba8(512, 512);
        let (composite, placed) = compose(&overview, Some(&roi));
        assert_eq!(composite.width(), 960 + SEPARATOR_PX + 512);
        assert_eq!(composite.height(), 540);
        assert_eq!(placed.width(), 960);
        assert_eq!(placed.height(), 540);
        // The ROI keeps its 100% scale in the composite.
        let (cw, ch) = composite.dimensions();
        assert_eq!(cw, 1480);
        assert_eq!(ch, 540);
    }

    #[test]
    fn compose_fits_pixel_budget_on_4k() {
        let overview = DynamicImage::new_rgba8(1920, 1080);
        let roi = DynamicImage::new_rgba8(512, 512);
        let (composite, placed) = compose(&overview, Some(&roi));
        let pixels = u64::from(composite.width()) * u64::from(composite.height());
        assert!(pixels <= MAX_COMPOSITE_PIXELS);
        assert!(placed.width() < 1920, "overview must shrink to make room");
        // The ROI is never shrunk.
        assert_eq!(composite.width() - placed.width() - SEPARATOR_PX, 512);
    }

    #[test]
    fn compose_fits_pixel_budget_without_roi_on_5k() {
        // 50% of a 5120x2880 display exceeds the runtime pixel budget; the
        // overview must be capped even without a ROI.
        let overview = DynamicImage::new_rgba8(2560, 1440);
        let (composite, placed) = compose(&overview, None);
        assert_eq!(composite.width(), placed.width());
        assert_eq!(composite.height(), placed.height());
        let pixels = u64::from(composite.width()) * u64::from(composite.height());
        assert!(pixels <= MAX_COMPOSITE_PIXELS);
    }

    #[test]
    fn compose_without_roi_passes_through_small_overview() {
        let overview = DynamicImage::new_rgba8(960, 540);
        let (composite, placed) = compose(&overview, None);
        assert_eq!(composite.width(), 960);
        assert_eq!(composite.height(), 540);
        assert_eq!(placed.as_bytes(), overview.as_bytes());
    }
}
