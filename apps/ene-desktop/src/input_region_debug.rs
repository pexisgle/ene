//! PR-LX.8: debug visualisation of the Linux input region.
//!
//! The Linux click-through pipeline pushes a set of rectangles
//! into the Wayland `wl_surface::set_input_region` and the
//! X11 `shape::rectangles` calls each frame. When the result
//! does not look right ("the character is click-through, but
//! the empty area around it is not — or vice versa") the
//! fastest way to debug it is to overlay the actual
//! rectangles the runtime just sent. This module owns that
//! overlay.
//!
//! F9 toggles the overlay (Linux only, not persisted). The
//! overlay reuses the existing collider debug renderer
//! (`DebugRenderer`) so no new GPU resources are required.
//!
//! # Colour coding
//!
//! - `Empty` (red): the runtime pushed an empty rectangle
//!   set. The window passes all clicks through to the
//!   desktop. We draw a single red border around the entire
//!   window so the user can see at a glance "nothing is
//!   clickable".
//! - `Freeze` (green): the user is holding the `F8` "freeze
//!   character window" hotkey. The window accepts all input
//!   regardless of cursor position. We draw a green border
//!   around the entire window.
//! - `FullWindow` (yellow): the runtime decided the cursor
//!   is on the silhouette (or the mask has no data) and
//!   pushed a full-window rectangle. Yellow border.
//! - `Mask` (orange): the runtime forwarded mask readback
//!   rectangles to the OS. Each rectangle is drawn as a
//!   4-segment wireframe in orange.
//!
//! The colours are deliberately distinct from the existing
//! collider overlay (cyan + magenta + red hit) and the mask
//! gizmo (purple), so all four overlays can be on at once
//! without visual confusion.

use crate::platform::wayland_region::Rect;

/// What the runtime pushed to the OS this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRegionSource {
    /// Empty rectangle set. All clicks pass through.
    Empty,
    /// `F8` freeze hotkey is held. All input accepted.
    Freeze,
    /// Full-window accept (cursor on silhouette, or mask had
    /// no data).
    FullWindow,
    /// Per-rectangle set forwarded from the mask readback.
    Mask,
}

const COLOR_EMPTY: [f32; 4] = [1.0, 0.2, 0.2, 0.9];
const COLOR_FREEZE: [f32; 4] = [0.3, 1.0, 0.3, 0.9];
const COLOR_FULL: [f32; 4] = [1.0, 0.95, 0.2, 0.9];
const COLOR_MASK: [f32; 4] = [1.0, 0.55, 0.0, 0.9];

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

/// Append 4-segment wireframe `DebugLine`s for each input
/// region rectangle to `out`.
///
/// `rects` is in **window-pixel** space (the same
/// coordinate system the OS receives). Negative or out-of-
/// window rectangles are clamped to the window so the
/// wireframe stays on-screen. The function handles the
/// `Empty` / `Freeze` / `FullWindow` modes by emitting a
/// single border rectangle around the entire window.
pub fn build_input_region_debug_lines(
    out: &mut Vec<ene_vrm::DebugLine>,
    rects: &[Rect],
    source: InputRegionSource,
    window_w: u32,
    window_h: u32,
    view_inverse: glam::Mat4,
    view_z: f32,
) {
    let (rects_to_draw, color) = match source {
        InputRegionSource::Empty => (
            vec![(0_i32, 0_i32, window_w as i32, window_h as i32)],
            COLOR_EMPTY,
        ),
        InputRegionSource::Freeze => (
            vec![(0_i32, 0_i32, window_w as i32, window_h as i32)],
            COLOR_FREEZE,
        ),
        InputRegionSource::FullWindow => (
            vec![(0_i32, 0_i32, window_w as i32, window_h as i32)],
            COLOR_FULL,
        ),
        InputRegionSource::Mask => (rects.to_vec(), COLOR_MASK),
    };

    for (x, y, w, h) in rects_to_draw {
        // Skip empty / zero-area rects (X11 / Wayland
        // silently drop these, drawing them would be
        // misleading).
        if w <= 0 || h <= 0 {
            continue;
        }
        // Clamp to window. Allow off-screen origins (negative
        // x/y) so we can still see the part that *is* on
        // screen.
        let min_x = x.max(0) as f32;
        let min_y = y.max(0) as f32;
        let max_x = (x + w).min(window_w as i32).max(0) as f32;
        let max_y = (y + h).min(window_h as i32).max(0) as f32;
        if max_x <= min_x || max_y <= min_y {
            continue;
        }

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
        let c = glam::Vec4::from(color);
        out.push(ene_vrm::DebugLine {
            a: p0,
            b: p1,
            color: c,
        });
        out.push(ene_vrm::DebugLine {
            a: p1,
            b: p2,
            color: c,
        });
        out.push(ene_vrm::DebugLine {
            a: p2,
            b: p3,
            color: c,
        });
        out.push(ene_vrm::DebugLine {
            a: p3,
            b: p0,
            color: c,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_draws_full_window_red_border() {
        let mut out = Vec::new();
        build_input_region_debug_lines(
            &mut out,
            &[],
            InputRegionSource::Empty,
            100,
            100,
            glam::Mat4::IDENTITY,
            -3.0,
        );
        // 4 segments for the window border.
        assert_eq!(out.len(), 4);
        for line in &out {
            assert_eq!(line.color, glam::Vec4::from(COLOR_EMPTY));
        }
    }

    #[test]
    fn freeze_source_uses_green_color() {
        let mut out = Vec::new();
        build_input_region_debug_lines(
            &mut out,
            &[],
            InputRegionSource::Freeze,
            200,
            200,
            glam::Mat4::IDENTITY,
            -3.0,
        );
        assert_eq!(out.len(), 4);
        for line in &out {
            assert_eq!(line.color, glam::Vec4::from(COLOR_FREEZE));
        }
    }

    #[test]
    fn mask_source_emits_four_segments_per_rect() {
        let rects = vec![(10_i32, 20_i32, 30_i32, 40_i32), (100, 100, 50, 50)];
        let mut out = Vec::new();
        build_input_region_debug_lines(
            &mut out,
            &rects,
            InputRegionSource::Mask,
            1000,
            1000,
            glam::Mat4::IDENTITY,
            -3.0,
        );
        // 2 rects × 4 segments.
        assert_eq!(out.len(), 8);
        for line in &out {
            assert_eq!(line.color, glam::Vec4::from(COLOR_MASK));
        }
    }

    #[test]
    fn out_of_window_rect_is_clamped() {
        let rects = vec![(-50_i32, -50_i32, 1_000_i32, 1_000_i32)];
        let mut out = Vec::new();
        build_input_region_debug_lines(
            &mut out,
            &rects,
            InputRegionSource::Mask,
            200,
            300,
            glam::Mat4::IDENTITY,
            -3.0,
        );
        assert_eq!(out.len(), 4);
        // All corners in world space, centred on the world
        // origin and spanning the full window. The 1000×1000
        // rect is clamped to the 200×300 window, so the
        // world-space x range is [-0.8666, 0.8666] and y range is
        // [-1.3, 1.3].
        let xs: Vec<f32> = out.iter().flat_map(|l| [l.a.x, l.b.x]).collect();
        let ys: Vec<f32> = out.iter().flat_map(|l| [l.a.y, l.b.y]).collect();
        for &x in &xs {
            assert!((-0.87..=0.87).contains(&x), "x={x} out of window");
        }
        for &y in &ys {
            assert!((-1.31..=1.31).contains(&y), "y={y} out of window");
        }
    }

    #[test]
    fn pixel_to_world_transform_uses_centering_and_y_flip() {
        // 100×100 window. The full-window Empty border
        // should be at world coordinates spanning
        // [-1.3, 1.3] × [-1.3, 1.3] (centred on the world
        // origin). z must be -3.0 (slightly in front of
        // the model so the depth test passes).
        let mut out = Vec::new();
        build_input_region_debug_lines(
            &mut out,
            &[],
            InputRegionSource::Empty,
            100,
            100,
            glam::Mat4::IDENTITY,
            -3.0,
        );
        assert_eq!(out.len(), 4);
        for line in &out {
            // Every vertex on the centred box.
            assert!(
                (-1.31..=1.31).contains(&line.a.x),
                "a.x={} out of range",
                line.a.x
            );
            assert!(
                (-1.31..=1.31).contains(&line.b.x),
                "b.x={} out of range",
                line.b.x
            );
            assert!(
                (-1.31..=1.31).contains(&line.a.y),
                "a.y={} out of range",
                line.a.y
            );
            assert!(
                (-1.31..=1.31).contains(&line.b.y),
                "b.y={} out of range",
                line.b.y
            );
            assert!((line.a.z - -3.0).abs() < 1e-6, "a.z={} not -3.0", line.a.z);
            assert!((line.b.z - -3.0).abs() < 1e-6, "b.z={} not -3.0", line.b.z);
        }
    }
}
