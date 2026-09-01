//! Per-body drag state machine and screen-space hit-testing for the overlay.

use std::collections::BTreeMap;

use glam::{Mat4, Vec2, Vec3, Vec3Swizzles};

/// Lower bound for a normalized body position.
pub const POSITION_MIN: f32 = 0.02;
/// Upper bound for a normalized body position.
pub const POSITION_MAX: f32 = 0.98;

/// Fallback normalized position for a body missing from the saved map.
pub const DEFAULT_BODY_POSITION: [f32; 2] = [0.78, 0.5];

/// Clamps a normalized position into the valid overlay range.
#[must_use]
pub fn clamp_position(pos: [f32; 2]) -> [f32; 2] {
    [
        clamp_axis(pos[0], DEFAULT_BODY_POSITION[0]),
        clamp_axis(pos[1], DEFAULT_BODY_POSITION[1]),
    ]
}

fn clamp_axis(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(POSITION_MIN, POSITION_MAX)
    } else {
        fallback
    }
}

fn aspect_from_viewport(viewport: (u32, u32)) -> f32 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "swapchain pixels are well inside f32"
    )]
    let aspect = viewport.0.max(1) as f32 / viewport.1.max(1) as f32;
    aspect.max(0.0001)
}

fn viewport_half_extents(aspect: f32) -> (f32, f32) {
    let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
    let half_w = half_h * aspect.max(0.0001);
    (half_w, half_h)
}

/// Maps a normalized position to a world-space XY offset on the
/// orthographic focal plane for the given viewport aspect.
#[must_use]
pub fn normalized_to_world(pos: [f32; 2], viewport: (u32, u32)) -> [f32; 2] {
    let (half_w, half_h) = viewport_half_extents(aspect_from_viewport(viewport));
    [(pos[0] - 0.5) * 2.0 * half_w, (0.5 - pos[1]) * 2.0 * half_h]
}

/// Maps a world-space XY offset back to clamped normalized coordinates.
#[must_use]
pub fn world_to_normalized(world: Vec2, viewport: (u32, u32)) -> [f32; 2] {
    let (half_w, half_h) = viewport_half_extents(aspect_from_viewport(viewport));
    clamp_position([
        world.x / (2.0 * half_w) + 0.5,
        0.5 - world.y / (2.0 * half_h),
    ])
}

/// One drawable body offered to the hit-test pass.
#[derive(Debug, Clone)]
pub struct HitCandidate {
    pub soul_id: String,
    /// World-space center of the body, used to break equal-depth ties.
    pub world_center: Vec3,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
}

/// Active drag gesture targeting one body.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyDrag {
    Armed { soul_id: String, grab_offset: Vec2 },
    Dragging { soul_id: String, grab_offset: Vec2 },
}

impl BodyDrag {
    #[must_use]
    pub fn soul_id(&self) -> &str {
        match self {
            Self::Armed { soul_id, .. } | Self::Dragging { soul_id, .. } => soul_id,
        }
    }

    #[must_use]
    pub const fn grab_offset(&self) -> Vec2 {
        match self {
            Self::Armed { grab_offset, .. } | Self::Dragging { grab_offset, .. } => *grab_offset,
        }
    }
}

/// Interaction inputs beyond surface transparency for [`allows_input`].
#[derive(Clone, Copy)]
pub struct OverlayInputState {
    /// Saved click-through preference while the overlay chrome is hidden.
    pub click_through_preferred: bool,
    /// A focused chrome window needs protection above the overlay.
    pub chrome_protected: bool,
    /// The cursor rests over a body silhouette.
    pub hovering_body: bool,
    /// A drag gesture is active and keeps receiving moves off-silhouette.
    pub dragging: bool,
}

/// Whether the overlay window should accept pointer input.
/// An opaque overlay always accepts input. A transparent overlay with
/// click-through preferred accepts input only when a chrome window holds
/// focus, the cursor rests on a body silhouette, or a drag gesture is active.
#[must_use]
pub fn allows_input(transparent: bool, state: OverlayInputState) -> bool {
    let OverlayInputState {
        click_through_preferred,
        chrome_protected,
        hovering_body,
        dragging,
    } = state;
    !(transparent && click_through_preferred) || chrome_protected || hovering_body || dragging
}

/// Arms a press on the hovered body, or clears any stale gesture when the
/// background was pressed. Takes the body's saved normalized position; the
/// grab offset keeps the cursor anchored where it grabbed the body.
pub fn press_body(
    drag: &mut Option<BodyDrag>,
    hit_soul: Option<&str>,
    stored: Option<[f32; 2]>,
    cursor_world: Option<Vec2>,
    viewport: (u32, u32),
) {
    let Some(soul_id) = hit_soul else {
        *drag = None;
        return;
    };
    let body_world = Vec2::from(normalized_to_world(
        stored.unwrap_or(DEFAULT_BODY_POSITION),
        viewport,
    ));
    let grab_offset = cursor_world.map_or(Vec2::ZERO, |world| world - body_world);
    *drag = Some(BodyDrag::Armed {
        soul_id: soul_id.to_owned(),
        grab_offset,
    });
}

/// Advances the gesture for a cursor-move event: the first move turns the
/// armed press into a drag, and every move repositions only the dragged body
/// while preserving its grab offset.
pub fn drag_body(
    drag: &mut Option<BodyDrag>,
    positions: &mut BTreeMap<String, [f32; 2]>,
    cursor_world: Option<Vec2>,
    viewport: (u32, u32),
) {
    if let Some(BodyDrag::Armed {
        soul_id,
        grab_offset,
    }) = drag.clone()
    {
        *drag = Some(BodyDrag::Dragging {
            soul_id,
            grab_offset,
        });
    }
    if let (
        Some(BodyDrag::Dragging {
            soul_id,
            grab_offset,
        }),
        Some(world),
    ) = (drag.as_ref(), cursor_world)
    {
        let pos = world_to_normalized(world - *grab_offset, viewport);
        positions.insert(soul_id.clone(), pos);
    }
}

/// Ends the gesture, returning the soul whose position must persist.
pub fn release_body(drag: &mut Option<BodyDrag>) -> Option<String> {
    drag.take().map(|gesture| gesture.soul_id().to_owned())
}

/// Transforms the eight corners of a local AABB by `model_mat`.
#[must_use]
pub fn aabb_world_corners(aabb_min: [f32; 3], aabb_max: [f32; 3], model_mat: Mat4) -> [Vec3; 8] {
    let [ax, ay, az] = aabb_min;
    let [bx, by, bz] = aabb_max;
    [
        Vec3::new(ax, ay, az),
        Vec3::new(ax, ay, bz),
        Vec3::new(ax, by, az),
        Vec3::new(ax, by, bz),
        Vec3::new(bx, ay, az),
        Vec3::new(bx, ay, bz),
        Vec3::new(bx, by, az),
        Vec3::new(bx, by, bz),
    ]
    .map(|corner| model_mat.transform_point3(corner))
}

/// Computes the world-space bounds of a transformed local AABB.
#[must_use]
pub fn transformed_aabb_bounds(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    model_mat: Mat4,
) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for corner in aabb_world_corners(aabb_min, aabb_max, model_mat) {
        min = min.min(corner);
        max = max.max(corner);
    }
    (min, max)
}

/// Slab-method ray/AABB intersection returning the entry distance along
/// the view ray. None when the ray misses or the box lies behind the origin.
#[must_use]
pub fn ray_aabb_entry(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let eps = 1e-6;
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;

    for (origin_axis, dir, lo, hi) in [
        (origin.x, direction.x, min.x, max.x),
        (origin.y, direction.y, min.y, max.y),
        (origin.z, direction.z, min.z, max.z),
    ] {
        if dir.abs() < eps {
            if origin_axis < lo || origin_axis > hi {
                return None;
            }
        } else {
            let inv = 1.0 / dir;
            let mut t1 = (lo - origin_axis) * inv;
            let mut t2 = (hi - origin_axis) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
        }
    }

    if t_min <= t_max && t_max >= 0.0 {
        Some(t_min.max(0.0))
    } else {
        None
    }
}

/// Projects a window-logical cursor position to world XY on the camera's
/// focal plane. Returns None for degenerate viewports.
#[must_use]
pub fn cursor_logical_to_world_2d(
    cursor_logical: Vec2,
    viewport: (u32, u32),
    camera_eye: Vec3,
    camera_target: Vec3,
    camera_up: Vec3,
) -> Option<Vec2> {
    if viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let ndc = ene_vrm::pixel_to_ndc(cursor_logical.x, cursor_logical.y, viewport);
    let view_pos = ene_vrm::ndc_to_view_pos(ndc, viewport, 0.0);
    let view = glam::camera::rh::view::look_at_mat4(camera_eye, camera_target, camera_up);
    let world = ene_vrm::view_pos_to_world(view_pos, view);
    Some(Vec2::new(world.x, world.y))
}

/// Finds the frontmost body whose world AABB contains the logical cursor.
///
/// Overlapping candidates resolve to the nearest along the view ray;
/// equidistant hits prefer the later index, matching draw order.
#[must_use]
pub fn hit_test(
    candidates: &[HitCandidate],
    viewport: (u32, u32),
    camera_eye: Vec3,
    camera_target: Vec3,
    camera_up: Vec3,
    cursor_logical: Vec2,
) -> Option<String> {
    if candidates.is_empty() || viewport.0 == 0 || viewport.1 == 0 {
        return None;
    }
    let ndc = ene_vrm::pixel_to_ndc(cursor_logical.x, cursor_logical.y, viewport);
    let view_pos = ene_vrm::ndc_to_view_pos(ndc, viewport, 0.0);
    let view = glam::camera::rh::view::look_at_mat4(camera_eye, camera_target, camera_up);
    let origin = ene_vrm::view_pos_to_world(view_pos, view);
    let direction = (camera_target - camera_eye).normalize();

    let cursor_world = {
        // Project the cursor onto the camera focal plane for center-distance
        // tie-breaking in orthographic view where all bodies share a depth.
        let view_pos = ene_vrm::ndc_to_view_pos(ndc, viewport, 0.0);
        let view = glam::camera::rh::view::look_at_mat4(camera_eye, camera_target, camera_up);
        let world = ene_vrm::view_pos_to_world(view_pos, view);
        Vec3::new(world.x, world.y, 0.0)
    };
    let mut best: Option<(f32, f32, &HitCandidate)> = None;
    for candidate in candidates {
        let Some(entry) = ray_aabb_entry(origin, direction, candidate.aabb_min, candidate.aabb_max)
        else {
            continue;
        };
        let center_dist = (candidate.world_center - cursor_world).xy().length();
        let replace = best.is_none_or(|(best_t, best_dist, _)| {
            const DEPTH_EPSILON: f32 = 1e-4;
            entry < best_t || ((entry - best_t).abs() < DEPTH_EPSILON && center_dist < best_dist)
        });
        if replace {
            best = Some((entry, center_dist, candidate));
        }
    }
    best.map(|(_, _, candidate)| candidate.soul_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EYE: Vec3 = Vec3::new(0.0, 0.3, 3.0);
    const TARGET: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    const TEST_VIEWPORT: (u32, u32) = (640, 480);

    fn candidate(soul: &str, min: Vec3, max: Vec3) -> HitCandidate {
        HitCandidate {
            soul_id: soul.to_owned(),
            world_center: (min + max) / 2.0,
            aabb_min: min,
            aabb_max: max,
        }
    }

    fn centered_candidate(soul: &str, half: f32) -> HitCandidate {
        candidate(soul, Vec3::splat(-half), Vec3::splat(half))
    }

    #[test]
    fn clamp_position_bounds_coordinates() {
        let [x, y] = clamp_position([0.01, 0.99]);
        assert!((x - POSITION_MIN).abs() < f32::EPSILON);
        assert!((y - POSITION_MAX).abs() < f32::EPSILON);
        let [mid_x, mid_y] = clamp_position([0.5, 0.5]);
        assert!((mid_x - 0.5).abs() < f32::EPSILON);
        assert!((mid_y - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn normalized_world_roundtrip() {
        let original = [0.7, 0.3];
        let world = normalized_to_world(original, TEST_VIEWPORT);
        let back = world_to_normalized(Vec2::from(world), TEST_VIEWPORT);
        assert!((back[0] - original[0]).abs() < 1e-5);
        assert!((back[1] - original[1]).abs() < 1e-5);
    }

    #[test]
    fn allows_input_combination_table() {
        let state = |click_through_preferred, chrome_protected, hovering_body, dragging| {
            OverlayInputState {
                click_through_preferred,
                chrome_protected,
                hovering_body,
                dragging,
            }
        };
        // Opaque always accepts input regardless of other flags.
        assert!(allows_input(false, state(true, false, false, false)));
        assert!(allows_input(false, state(false, false, false, false)));
        // Transparent with click-through and nothing special: blocked.
        assert!(!allows_input(true, state(true, false, false, false)));
        // Hovering a body opens a hole in the click-through.
        assert!(allows_input(true, state(true, false, true, false)));
        // An active drag keeps receiving move events outside the silhouette.
        assert!(allows_input(true, state(true, false, false, true)));
        // Chrome protection takes priority over click-through.
        assert!(allows_input(true, state(true, true, false, false)));
        // Transparent but click-through not preferred: always allowed.
        assert!(allows_input(true, state(false, false, false, false)));
    }

    #[test]
    fn aabb_corners_follow_translation() {
        let mat = Mat4::from_translation(Vec3::new(1.0, 2.0, 0.0));
        let corners = aabb_world_corners([-0.5, 0.0, -0.3], [0.5, 1.4, 0.3], mat);
        let min = corners
            .iter()
            .copied()
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let max = corners
            .iter()
            .copied()
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        assert_eq!(min, Vec3::new(0.5, 2.0, -0.3));
        assert_eq!(max, Vec3::new(1.5, 3.4, 0.3));
    }

    #[test]
    fn transformed_bounds_identity_returns_local() {
        let (min, max) =
            transformed_aabb_bounds([-0.5, 0.0, -0.3], [0.5, 1.4, 0.3], Mat4::IDENTITY);
        assert_eq!(min, Vec3::new(-0.5, 0.0, -0.3));
        assert_eq!(max, Vec3::new(0.5, 1.4, 0.3));
    }

    #[test]
    fn ray_hits_and_misses_box() {
        let min = Vec3::new(-0.5, 0.0, -0.3);
        let max = Vec3::new(0.5, 1.4, 0.3);
        let hit = ray_aabb_entry(
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::new(0.0, 0.0, -1.0),
            min,
            max,
        );
        assert!(hit.is_some());
        let miss_x = ray_aabb_entry(
            Vec3::new(10.0, 1.0, 5.0),
            Vec3::new(0.0, 0.0, -1.0),
            min,
            max,
        );
        assert!(miss_x.is_none());
        let miss_y = ray_aabb_entry(
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            min,
            max,
        );
        assert!(miss_y.is_none());
    }

    #[test]
    fn cursor_projection_center_matches_eye_plane() {
        let world =
            cursor_logical_to_world_2d(Vec2::new(320.0, 240.0), (640, 480), EYE, TARGET, UP)
                .unwrap_or_default();
        assert!(world.x.abs() < 1e-3, "x = {world}");
        assert!((world.y - 0.3).abs() < 1e-3, "y = {}", world.y);
    }

    #[test]
    fn cursor_projection_delta_proportional_to_pixels() {
        let first =
            cursor_logical_to_world_2d(Vec2::new(100.0, 100.0), (640, 480), EYE, TARGET, UP)
                .unwrap_or_default();
        let second =
            cursor_logical_to_world_2d(Vec2::new(110.0, 100.0), (640, 480), EYE, TARGET, UP)
                .unwrap_or_default();
        let delta = second - first;
        assert!(delta.x > 0.0);
        assert!(delta.y.abs() < 1e-3);
    }

    #[test]
    fn degenerate_viewport_projects_nothing() {
        assert!(
            cursor_logical_to_world_2d(Vec2::ZERO, (0, 0), EYE, TARGET, UP).is_none(),
            "zero viewport must not project"
        );
    }

    #[test]
    fn hit_test_center_of_single_body() {
        let candidates = vec![centered_candidate("soul-a", 0.5)];
        let hit = hit_test(
            &candidates,
            (640, 480),
            EYE,
            TARGET,
            UP,
            Vec2::new(320.0, 240.0),
        );
        assert_eq!(hit.as_deref(), Some("soul-a"));
    }

    #[test]
    fn hit_test_misses_outside_corner() {
        let candidates = vec![candidate(
            "soul-a",
            Vec3::new(-0.1, -0.1, -0.1),
            Vec3::new(0.1, 0.1, 0.1),
        )];
        let hit = hit_test(
            &candidates,
            (640, 480),
            EYE,
            TARGET,
            UP,
            Vec2::new(5.0, 5.0),
        );
        assert!(hit.is_none());
    }

    #[test]
    fn hit_test_respects_viewport_aspect() {
        // A tall thin box: easy to hit on a square viewport but the narrow x
        // extent misses once the viewport widens.
        let candidates = vec![candidate(
            "tall",
            Vec3::new(-0.05, -0.5, -0.05),
            Vec3::new(0.05, 0.5, 0.05),
        )];
        let square_hit = hit_test(
            &candidates,
            (400, 400),
            EYE,
            TARGET,
            UP,
            Vec2::new(200.0, 200.0),
        );
        assert_eq!(square_hit.as_deref(), Some("tall"));
        let wide_miss = hit_test(
            &candidates,
            (800, 400),
            EYE,
            TARGET,
            UP,
            Vec2::new(100.0, 200.0),
        );
        assert!(wide_miss.is_none());
    }

    #[test]
    fn hit_test_prefers_nearest_body_on_overlap() {
        // Camera looks from +z toward origin so the higher-z box is closer.
        let near = candidate("near", Vec3::splat(-0.3), Vec3::new(0.3, 0.3, 1.0));
        let far = candidate("far", Vec3::new(-0.3, -0.3, -1.0), Vec3::splat(0.3));
        let candidates = vec![far, near];
        let hit = hit_test(
            &candidates,
            (640, 480),
            EYE,
            TARGET,
            UP,
            Vec2::new(320.0, 240.0),
        );
        assert_eq!(hit.as_deref(), Some("near"));
    }

    #[test]
    fn hit_test_prefers_nearest_center_at_equal_depth() {
        let candidates = vec![
            candidate(
                "centered",
                Vec3::new(-0.4, -0.5, -0.1),
                Vec3::new(0.4, 0.5, 0.1),
            ),
            candidate(
                "offset",
                Vec3::new(-0.2, -0.5, -0.1),
                Vec3::new(0.6, 0.5, 0.1),
            ),
        ];
        let hit = hit_test(
            &candidates,
            (640, 480),
            EYE,
            TARGET,
            UP,
            Vec2::new(320.0, 240.0),
        );
        // Cursor sits at world origin; "centered" spans it symmetrically while
        // "offset" centers at +0.2, so proximity picks the centered body.
        assert_eq!(hit.as_deref(), Some("centered"));
    }

    #[test]
    fn body_drag_accessors_expose_target_and_grab() {
        let drag = BodyDrag::Dragging {
            soul_id: "abc".to_owned(),
            grab_offset: Vec2::new(0.1, 0.2),
        };
        assert_eq!(drag.soul_id(), "abc");
        assert_eq!(drag.grab_offset(), Vec2::new(0.1, 0.2));
    }

    #[test]
    fn body_drag_equality_covers_variant_and_payload() {
        let left = BodyDrag::Armed {
            soul_id: "a".to_owned(),
            grab_offset: Vec2::ZERO,
        };
        let right = BodyDrag::Armed {
            soul_id: "a".to_owned(),
            grab_offset: Vec2::ZERO,
        };
        let other = BodyDrag::Dragging {
            soul_id: "a".to_owned(),
            grab_offset: Vec2::ZERO,
        };
        assert_eq!(left, right);
        assert_ne!(left, other);
    }

    #[test]
    fn press_on_background_clears_gesture() {
        let mut drag = Some(BodyDrag::Dragging {
            soul_id: "stale".to_owned(),
            grab_offset: Vec2::ONE,
        });
        press_body(&mut drag, None, Some([0.5, 0.5]), Some(Vec2::ZERO), TEST_VIEWPORT);
        assert!(drag.is_none());
    }

    #[test]
    fn press_on_body_arms_with_grab_offset() {
        let mut drag = None;
        let stored = [0.6, 0.4];
        let body_world = Vec2::from(normalized_to_world(stored, TEST_VIEWPORT));
        let cursor_world = body_world + Vec2::new(0.1, -0.05);
        press_body(
            &mut drag,
            Some("soul-a"),
            Some(stored),
            Some(cursor_world),
            TEST_VIEWPORT,
        );
        let armed = drag.expect("armed");
        assert_eq!(armed.soul_id(), "soul-a");
        let grab = armed.grab_offset();
        assert!((grab.x - 0.1).abs() < 1e-5);
        assert!((grab.y + 0.05).abs() < 1e-5);
    }

    #[test]
    fn first_move_promotes_armed_to_dragging_and_moves_only_target() {
        let mut positions =
            BTreeMap::from([("a".to_owned(), [0.3, 0.5]), ("b".to_owned(), [0.7, 0.5])]);
        let mut drag = Some(BodyDrag::Armed {
            soul_id: "a".to_owned(),
            grab_offset: Vec2::ZERO,
        });
        let world = Vec2::new(-0.2, 0.1);
        drag_body(&mut drag, &mut positions, Some(world), TEST_VIEWPORT);
        assert_eq!(
            drag,
            Some(BodyDrag::Dragging {
                soul_id: "a".to_owned(),
                grab_offset: Vec2::ZERO,
            }),
        );
        let moved = positions["a"];
        let expected = world_to_normalized(world, TEST_VIEWPORT);
        assert!((moved[0] - expected[0]).abs() < 1e-5);
        assert!((moved[1] - expected[1]).abs() < 1e-5);
        let [b_x, b_y] = positions["b"];
        assert!((b_x - 0.7).abs() < f32::EPSILON);
        assert!((b_y - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn dragging_keeps_grab_offset_between_moves() {
        let mut positions = BTreeMap::new();
        positions.insert("a".to_owned(), [0.5, 0.5]);
        let grab = Vec2::new(0.02, -0.03);
        let mut drag = Some(BodyDrag::Dragging {
            soul_id: "a".to_owned(),
            grab_offset: grab,
        });
        let first = Vec2::new(0.05, -0.04);
        drag_body(&mut drag, &mut positions, Some(first), TEST_VIEWPORT);
        let second = Vec2::new(0.09, -0.08);
        drag_body(&mut drag, &mut positions, Some(second), TEST_VIEWPORT);
        // The offset keeps the body anchored to the cursor across moves.
        let pos = positions["a"];
        let anchored = second - grab;
        assert!((normalized_to_world(pos, TEST_VIEWPORT)[0] - anchored.x).abs() < 1e-5);
        assert!((normalized_to_world(pos, TEST_VIEWPORT)[1] - anchored.y).abs() < 1e-5);
    }

    #[test]
    fn release_returns_dragged_soul_and_clears_state() {
        let mut drag = Some(BodyDrag::Dragging {
            soul_id: "abc".to_owned(),
            grab_offset: Vec2::ZERO,
        });
        assert_eq!(release_body(&mut drag).as_deref(), Some("abc"));
        assert!(drag.is_none());
        assert!(release_body(&mut drag).is_none());
    }

    #[test]
    fn dragged_position_stays_within_clamp_bounds() {
        let mut positions = BTreeMap::new();
        positions.insert("a".to_owned(), [0.5, 0.5]);
        let mut drag = Some(BodyDrag::Dragging {
            soul_id: "a".to_owned(),
            grab_offset: Vec2::ZERO,
        });
        drag_body(
            &mut drag,
            &mut positions,
            Some(Vec2::new(100.0, 100.0)),
            TEST_VIEWPORT,
        );
        let [x, y] = positions["a"];
        // World +y maps to normalized top (y minimum); +x maps to x maximum.
        assert!((y - POSITION_MIN).abs() < f32::EPSILON);
        assert!((x - POSITION_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn non_finite_saved_position_returns_to_the_default_slot() {
        let [x, y] = clamp_position([f32::NAN, f32::INFINITY]);
        assert!((x - DEFAULT_BODY_POSITION[0]).abs() < f32::EPSILON);
        assert!((y - DEFAULT_BODY_POSITION[1]).abs() < f32::EPSILON);
    }
}
