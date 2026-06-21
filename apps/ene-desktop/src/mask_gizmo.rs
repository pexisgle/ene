//! Mask capture debug overlay.
use crate::platform::wayland_mask_capture::MaskCaptureState;
use ene_vrm::debug_renderer::DebugLine;

pub const MASK_RECT_COLOR: glam::Vec4 = glam::Vec4::new(0.8, 0.3, 0.95, 0.7);

fn pixel_to_world(
    px: f32,
    py: f32,
    window_w: f32,
    window_h: f32,
    view_inverse: glam::Mat4,
    view_z: f32,
) -> glam::Vec3 {
    let ndc_x = (px / window_w) * 2.0 - 1.0;
    let ndc_y = -((py / window_h) * 2.0 - 1.0);
    let aspect = (window_w / window_h).max(0.0001);
    let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
    let half_w = half_h * aspect;
    let view_pos = glam::Vec3::new(ndc_x * half_w, ndc_y * half_h, view_z);
    view_inverse.transform_point3(view_pos)
}

/// Build the line list for the mask-capture debug overlay.
pub fn build_mask_rect_lines(
    out: &mut Vec<DebugLine>,
    mask: &MaskCaptureState,
    window_w: u32,
    window_h: u32,
    view_inverse: glam::Mat4,
    view_z: f32,
) {
    let guard = mask.lock();
    let rects = guard.extract_rectangles();
    let downsample = guard.downsample();
    drop(guard);
    if rects.is_empty() || downsample == 0 || window_w == 0 || window_h == 0 {
        return;
    }

    let dw = downsample as f32;
    for (rx, ry, rw, rh) in rects {
        let min_x = rx as f32 * dw;
        let min_y = ry as f32 * dw;
        let max_x = (rx + rw) as f32 * dw;
        let max_y = (ry + rh) as f32 * dw;

        let p0 = pixel_to_world(
            min_x,
            min_y,
            window_w as f32,
            window_h as f32,
            view_inverse,
            view_z,
        );
        let p1 = pixel_to_world(
            max_x,
            min_y,
            window_w as f32,
            window_h as f32,
            view_inverse,
            view_z,
        );
        let p2 = pixel_to_world(
            max_x,
            max_y,
            window_w as f32,
            window_h as f32,
            view_inverse,
            view_z,
        );
        let p3 = pixel_to_world(
            min_x,
            max_y,
            window_w as f32,
            window_h as f32,
            view_inverse,
            view_z,
        );

        out.push(DebugLine {
            a: p0,
            b: p1,
            color: MASK_RECT_COLOR,
        });
        out.push(DebugLine {
            a: p1,
            b: p2,
            color: MASK_RECT_COLOR,
        });
        out.push(DebugLine {
            a: p2,
            b: p3,
            color: MASK_RECT_COLOR,
        });
        out.push(DebugLine {
            a: p3,
            b: p0,
            color: MASK_RECT_COLOR,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_rect_color_is_purple_70pct() {
        assert_eq!(MASK_RECT_COLOR.x, 0.8);
        assert_eq!(MASK_RECT_COLOR.y, 0.3);
        assert_eq!(MASK_RECT_COLOR.z, 0.95);
        assert_eq!(MASK_RECT_COLOR.w, 0.7);
    }
}
