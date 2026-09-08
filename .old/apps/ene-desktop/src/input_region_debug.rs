//! Debug visualisation of the Linux input region.
//!
//! The Linux click-through pipeline pushes a set of rectangles
//! into the Wayland `wl_surface::set_input_region` and the
//! X11 `shape::rectangles` calls each frame. When the result
//! does not look right ("the character is click-through, but the
//! empty area around it is not — or vice versa") the fastest way
//! to debug it is to overlay the actual rectangles the runtime
//! just sent. F9 toggles the overlay (Linux only, not persisted).
//!
//! # Colour coding
//!
//! - `Empty` (red): empty rectangle set — full pass-through.
//!   Draws a red border around the entire window.
//! - `Freeze` (green): the `F8` freeze hotkey is held — all
//!   input accepted. Green border around the window.
//! - `FullWindow` (yellow): the cursor is on the silhouette
//!   (or the mask has no data) and a full-window rect was
//!   pushed. Yellow border.
//! - `Mask` (orange): mask-readback rectangles forwarded to
//!   the OS. Each rectangle is drawn as a 4-segment wireframe.
//!
//! The colours are deliberately distinct from the collider
//! overlay (cyan + magenta + red hit) and the mask gizmo
//! (purple), so all four overlays can be on at once without
//! visual confusion.

use crate::platform::wayland_region::Rect;

/// What the runtime pushed to the OS this frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum InputRegionSource {
    #[default]
    Empty,
    Freeze,
    FullWindow,
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
    let viewport = (window_w as u32, window_h as u32);
    let ndc = ene_vrm::pixel_to_ndc(px, py, viewport);
    let view_pos = ene_vrm::ndc_to_view_pos(ndc, viewport, view_z);
    ene_vrm::view_pos_to_world(view_pos, view_inverse)
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
            vec![Rect::new(0, 0, window_w as i32, window_h as i32)],
            COLOR_EMPTY,
        ),
        InputRegionSource::Freeze => (
            vec![Rect::new(0, 0, window_w as i32, window_h as i32)],
            COLOR_FREEZE,
        ),
        InputRegionSource::FullWindow => (
            vec![Rect::new(0, 0, window_w as i32, window_h as i32)],
            COLOR_FULL,
        ),
        InputRegionSource::Mask => (rects.to_vec(), COLOR_MASK),
    };

    for r in rects_to_draw {
        // X11 / Wayland silently drop zero-area rects;
        // drawing them would be misleading.
        if r.is_empty() {
            continue;
        }
        // Clamp to window. Allow off-screen origins so the
        // visible part still draws.
        let clamped = r.clamp_to(window_w as i32, window_h as i32);
        if clamped.is_empty() {
            continue;
        }
        let min_x = clamped.x as f32;
        let min_y = clamped.y as f32;
        let max_x = (clamped.x + clamped.w) as f32;
        let max_y = (clamped.y + clamped.h) as f32;

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
        let rects = vec![Rect::new(10, 20, 30, 40), Rect::new(100, 100, 50, 50)];
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
        assert_eq!(out.len(), 8);
        for line in &out {
            assert_eq!(line.color, glam::Vec4::from(COLOR_MASK));
        }
    }

    #[test]
    fn out_of_window_rect_is_clamped() {
        let rects = vec![Rect::new(-50, -50, 1_000, 1_000)];
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
