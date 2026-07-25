//! Region-of-interest cropping around the cursor for high-precision vision (#215).
//!
//! When enabled, the proactive observer crops a 512×512 region around the
//! cursor position at 100% scale before passing the image to the local vision
//! model. This gives the model higher-resolution detail around the user's
//! point of attention.

use image::{DynamicImage, GenericImageView};

/// Crop a 512×512 region around `(cursor_x, cursor_y)` at full resolution.
///
/// Coordinates are expected to be **global screen coordinates** as reported by
/// `device_query::DeviceState::get_mouse()`.
///
/// When the cursor is near an edge the crop is clamped so it stays within the
/// image bounds; the returned region may be smaller than 512×512 for corner
/// positions. Returns `None` when the image is empty.
pub fn crop_roi(full_image: &DynamicImage, cursor_x: i32, cursor_y: i32) -> Option<DynamicImage> {
    let (w, h) = full_image.dimensions();
    if w < 1 || h < 1 {
        return None;
    }

    let roi_size = 512u32;

    let cx = (cursor_x as u32).clamp(0, w.saturating_sub(1));
    let cy = (cursor_y as u32).clamp(0, h.saturating_sub(1));

    let x = cx
        .saturating_sub(roi_size / 2)
        .min(w.saturating_sub(roi_size));
    let y = cy
        .saturating_sub(roi_size / 2)
        .min(h.saturating_sub(roi_size));

    let roi_w = roi_size.min(w - x);
    let roi_h = roi_size.min(h - y);

    Some(full_image.crop_imm(x, y, roi_w, roi_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_center_of_large_image() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, 960, 540).expect("should crop");
        // 512x512 fits comfortably inside 1920x1080
        assert_eq!(cropped.width(), 512);
        assert_eq!(cropped.height(), 512);
    }

    #[test]
    fn crop_near_top_left_corner() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, 10, 10).expect("should crop");
        // Clamped: x=0, y=0, roi fits 512x512
        assert_eq!(cropped.width(), 512);
        assert_eq!(cropped.height(), 512);
    }

    #[test]
    fn crop_near_bottom_right_corner() {
        let img = DynamicImage::new_rgba8(1920, 1080);
        let cropped = crop_roi(&img, 1900, 1060).expect("should crop");
        // Clamped so it stays within bounds
        assert!(cropped.width() <= 512);
        assert!(cropped.height() <= 512);
        assert!(cropped.width() > 0);
        assert!(cropped.height() > 0);
    }

    #[test]
    fn crop_empty_image_returns_none() {
        let img = DynamicImage::new_rgba8(0, 0);
        assert!(crop_roi(&img, 0, 0).is_none());
    }
}
