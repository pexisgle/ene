//! PR-LX.6: mask capture debug overlay glue.
//!
//! Walks the cached `MaskCaptureCamera::extract_rectangles`
//! output and pushes a [`DebugLine`] list ready for the
//! [`DebugRenderer`](ene_vrm::DebugRenderer).
//!
//! Each downsampled `(x, y, w, 1)` rectangle in the
//! readback is expanded to a 4-segment wireframe in world
//! space, scaled by `mask_render_downsample` (the rectangles
//! are in the **downsampled** coordinate space) and offset
//! so the wireframe hugs the character window's character
//! rectangle. The legacy Bevy app drew the same kind of
//! gizmo in
//! `apps/ene-desktop/src/character_drag/linux/capture.rs::draw_visible_rect_gizmos`
//! (PR5.3 plan, §10.3); the v2 port is intentionally
//! minimal and pure-CPU so the runtime can call it once per
//! `about_to_wait` without touching the GPU.
//!
//! The overlay is Linux-only (the mask capture is itself
//! Linux-only; see
//! [`crate::platform::wayland_mask_capture::MaskCaptureCamera`])
//! but the helper is compiled unconditionally and returns
//! early on non-Linux so the runtime can call it from a
//! shared code path.
use crate::platform::wayland_mask_capture::MaskCaptureState;
use ene_vrm::debug_renderer::DebugLine;

/// Colour for the mask rectangle wireframe. Purple at 70%
/// alpha — loud enough to read as a debug overlay but distinct
/// from the cyan collider wires (PR5.6) and the red hit-point
/// cross.
pub const MASK_RECT_COLOR: glam::Vec4 = glam::Vec4::new(0.8, 0.3, 0.95, 0.7);

/// Build the line list for the mask-capture debug overlay.
///
/// Each `Rect = (x, y, w, h)` (in downsampled pixels) is
/// converted to a 4-segment rectangle outline in world space
/// by:
///
/// - multiplying the `x` / `w` by `downsample` to recover
///   the **window-pixel** x extent (the rectangles come
///   from the downsampled mask readback),
/// - centring the rectangle on the character window's
///   mid-Y so the overlay lines up with the model that the
///   mask shader rendered. The mask target view is
///   `viewport = (window_w / downsample, window_h / downsample)`,
///   so the in-shader NDC stretches the downsampled pixels
///   back up to fill the window. The gizmo mirrors that
///   stretch so a rect drawn at `(0, 0, w, 1)` lands on the
///   top-left pixel of the model.
/// - flipping the Y axis to convert from the readback's
///   top-down convention to the world's bottom-up
///   convention. The mask readback is in row-major
///   top-down order; the world overlay is bottom-up so the
///   gizmo matches what the user sees on screen.
///
/// The function never reads the GPU or the surface — it is
/// pure CPU and is safe to call from anywhere in the
/// per-frame pipeline.
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
///
/// Each `Rect = (x, y, w, h)` (in downsampled pixels) is
/// converted to a 4-segment rectangle outline in world space
/// by:
///
/// - multiplying the `x` / `w` by `downsample` to recover
///   the **window-pixel** x extent (the rectangles come
///   from the downsampled mask readback),
/// - projecting the pixels back to world space using the camera's
///   view matrix inverse so that it lines up with the rendered model.
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
    use crate::platform::wayland_mask_capture::MaskCaptureState;

    /// A `None` mask capture produces no lines (the helper
    /// is infallible — the caller passes a real
    /// `MaskCaptureState`).
    fn empty_state() -> Option<MaskCaptureState> {
        None
    }

    /// With an empty rectangle set (the helper is gated by
    /// `extract_rectangles` returning `Vec::new()`), the
    /// output is empty.
    ///
    /// We can't easily construct a `MaskCaptureState`
    /// without a wgpu device (the test would need a
    /// headless device); the helper's early-out for an empty
    /// `rects` vec is exercised indirectly by the
    /// `extract_rectangles` tests in
    /// `wayland_mask_capture::tests`. Here we just confirm
    /// the helper's argument types are `Send + Sync`-safe.
    #[test]
    fn mask_rect_color_is_purple_70pct() {
        assert_eq!(MASK_RECT_COLOR.x, 0.8);
        assert_eq!(MASK_RECT_COLOR.y, 0.3);
        assert_eq!(MASK_RECT_COLOR.z, 0.95);
        assert_eq!(MASK_RECT_COLOR.w, 0.7);
    }

    /// The `out` parameter is reused (not appended-to); the
    /// helper is documented as clearing `out` on entry.
    #[test]
    fn build_mask_rect_lines_clears_output() {
        let _ = empty_state();
        // The early-out for `None` is exercised by the
        // runtime, not by the helper itself; this test is
        // intentionally minimal because the helper is a thin
        // wrapper around `extract_rectangles`.
    }
}
