//! Region-of-interest cropping around the cursor for high-precision vision.
//!
//! The proactive observer crops a 512×512 region around the cursor at 100%
//! scale from the full-resolution capture and composites it next to the 50%
//! overview. This gives the model higher-resolution detail around the user's
//! point of attention.

use image::{DynamicImage, GenericImageView};

/// Side length of the 100%-scale crop around the cursor. Fixed across display
/// resolutions because glyph pixel density at 100% scale is what matters for
/// the vision model, not the fraction of the screen covered.
const ROI_SIZE_PX: u32 = 512;

/// Cursor anchors snap to this grid so small pointer moves keep the crop
/// pixel-identical and the screen-diff gate can reuse its cached summary.
const ANCHOR_GRID_PX: i32 = 64;

/// Crop a [`ROI_SIZE_PX`]-square region around `cursor` from `full`, whose
/// top-left corner sits at global `origin` (window or monitor position).
///
/// Coordinates are global screen coordinates as reported by
/// `device_query::DeviceState::get_mouse()`. The anchor is quantized to
/// [`ANCHOR_GRID_PX`]. When the cursor is near an edge the crop is clamped so
/// it stays within the image bounds; the returned region may be smaller than
/// [`ROI_SIZE_PX`]. Returns `None` when the image is empty or the cursor lies
/// outside the captured surface.
pub fn crop_roi(
    full_image: &DynamicImage,
    cursor: (i32, i32),
    origin: (i32, i32),
) -> Option<DynamicImage> {
    let (w, h) = full_image.dimensions();
    if w == 0 || h == 0 {
        return None;
    }

    let local_x = cursor.0.checked_sub(origin.0)?;
    let local_y = cursor.1.checked_sub(origin.1)?;
    if local_x < 0 || local_y < 0 || local_x >= w as i32 || local_y >= h as i32 {
        return None;
    }

    let anchor_x = local_x / ANCHOR_GRID_PX * ANCHOR_GRID_PX + ANCHOR_GRID_PX / 2;
    let anchor_y = local_y / ANCHOR_GRID_PX * ANCHOR_GRID_PX + ANCHOR_GRID_PX / 2;

    let half = ROI_SIZE_PX as i32 / 2;
    let max_x = (w as i32).saturating_sub(ROI_SIZE_PX as i32).max(0);
    let max_y = (h as i32).saturating_sub(ROI_SIZE_PX as i32).max(0);
    let x = (anchor_x - half).clamp(0, max_x);
    let y = (anchor_y - half).clamp(0, max_y);

    let roi_w = ROI_SIZE_PX.min(w - x as u32);
    let roi_h = ROI_SIZE_PX.min(h - y as u32);
    Some(full_image.crop_imm(x as u32, y as u32, roi_w, roi_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_center_of_large_image() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, (960, 540), (0, 0)).expect("should crop");
        // 512x512 fits comfortably inside 1920x1080
        assert_eq!(cropped.width(), 512);
        assert_eq!(cropped.height(), 512);
    }

    #[test]
    fn crop_near_top_left_corner() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, (10, 10), (0, 0)).expect("should crop");
        // Clamped: x=0, y=0, roi fits 512x512
        assert_eq!(cropped.width(), 512);
        assert_eq!(cropped.height(), 512);
    }

    #[test]
    fn crop_near_bottom_right_corner() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, (1900, 1060), (0, 0)).expect("should crop");
        // Clamped so it stays within bounds
        assert!(cropped.width() <= 512);
        assert!(cropped.height() <= 512);
        assert!(cropped.width() > 0);
        assert!(cropped.height() > 0);
    }

    #[test]
    fn crop_empty_image_returns_none() {
        let img = DynamicImage::new_rgba8(0, 0);
        assert!(crop_roi(&img, (0, 0), (0, 0)).is_none());
    }

    #[test]
    fn crop_translates_global_origin() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        // Window at (100, 50); cursor at global (110, 60) lands at (10, 10).
        let cropped = crop_roi(&img, (110, 60), (100, 50)).expect("should crop");
        assert_eq!(cropped.width(), 512);
        assert_eq!(cropped.height(), 512);
    }

    #[test]
    fn crop_rejects_cursor_outside_surface() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        assert!(crop_roi(&img, (2000, 500), (0, 0)).is_none());
        assert!(crop_roi(&img, (100, 1200), (0, 0)).is_none());
        assert!(crop_roi(&img, (-10, 500), (0, 0)).is_none());
    }

    #[test]
    fn crop_rejects_cursor_left_of_window_on_second_monitor() {
        // Multi-monitor: window starts at x=-1920; cursor at -1900 is inside,
        // cursor at 100 is outside.
        let img = DynamicImage::new_rgba8(1920, 1080);
        assert!(crop_roi(&img, (-1900, 500), (-1920, 0)).is_some());
        assert!(crop_roi(&img, (100, 500), (-1920, 0)).is_none());
    }

    #[test]
    fn anchor_quantization_keeps_crop_stable_within_cell() {
        let img = striped_image();
        let a = crop_roi(&img, (100, 100), (0, 0)).expect("should crop");
        let b = crop_roi(&img, (105, 110), (0, 0)).expect("should crop");
        assert_eq!(
            a.as_bytes(),
            b.as_bytes(),
            "same 64px cell must crop identically"
        );
    }

    #[test]
    fn anchor_quantization_changes_crop_across_cell_boundary() {
        let img = striped_image();
        let a = crop_roi(&img, (300, 400), (0, 0)).expect("should crop");
        let b = crop_roi(&img, (370, 400), (0, 0)).expect("should crop");
        assert_ne!(
            a.as_bytes(),
            b.as_bytes(),
            "new 64px cell must crop differently"
        );
    }

    #[test]
    fn crop_small_window_clamps_to_bounds() {
        let img = DynamicImage::new_rgba8(300, 200);
        let cropped = crop_roi(&img, (150, 100), (0, 0)).expect("should crop");
        assert_eq!(cropped.width(), 300);
        assert_eq!(cropped.height(), 200);
    }

    /// 1920x1080 image with a white blob near the tested anchors, so crops at
    /// different offsets are distinguishable (a uniform image cannot be).
    fn striped_image() -> DynamicImage {
        let mut img = DynamicImage::new_rgba8(1920, 1080);
        let pixels = img.as_mut_rgba8().expect("rgba image");
        for y in 300..400 {
            for x in 300..320 {
                pixels.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        img
    }
}
