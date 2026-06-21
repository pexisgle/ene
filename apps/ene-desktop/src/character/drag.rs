//! Drag-to-move for the character window.
//!
//! Coordinates cursor input, projects 2D screen positions into 3D world space,
//! and updates the character position. Supports click-through logic by integrating
//! with collision detection.
use glam::{Mat4, Vec2, Vec3};

/// Per-window drag state. `None` = idle, `Some(_)` = dragging.
///
/// Mirrors the legacy `CharacterDragState { last_cursor_world_pos:
/// Option<Vec2> }` exactly — the field name is preserved so the
/// `is_dragging()` semantics are unambiguous.
#[derive(Debug, Default, Clone)]
pub struct CharacterDragState {
    /// Last cursor world position during an active drag. `None` when
    /// the user is not currently dragging the character.
    pub last_cursor_world_pos: Option<Vec2>,
}

impl CharacterDragState {
    /// `true` while the user is holding left-mouse-button and
    /// dragging the character. The Windows click-through hook reads
    /// this so a drag in progress keeps the window receiving input
    /// even when the cursor has left the silhouette.
    pub fn is_dragging(&self) -> bool {
        self.last_cursor_world_pos.is_some()
    }
}

/// Mouse button event we care about for the state machine. Other
/// buttons (`Right`, `Middle`, `Other(_)`) are ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragButtonEvent {
    Pressed,
    Released,
}

/// What the runtime should do after feeding the state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum DragAction {
    /// No state change. The runtime should not touch
    /// `settings.character_state.character_position` or call
    /// `mark_dirty()`.
    None,
    /// The user just clicked-down on the character. State is now
    /// `Dragging`; the next `tick` will start producing deltas. No
    /// position change is required on this event.
    Started,
    /// The user released the mouse button after a drag. The runtime
    /// should call `settings.mark_dirty()` to persist the new
    /// position.
    Ended,
}

/// Transform the 8 AABB corners by the model matrix. 1:1 with
/// `legacy::aabb_world_corners`.
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
    .map(|c| model_mat.transform_point3(c))
}

/// Expand a model-local AABB into a world-space AABB after applying
/// the model matrix. 1:1 with `legacy::transformed_aabb_bounds`.
pub fn transformed_aabb_bounds(
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    model_mat: Mat4,
) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for c in aabb_world_corners(aabb_min, aabb_max, model_mat) {
        min = min.min(c);
        max = max.max(c);
    }
    (min, max)
}

/// Fast slab test against an axis-aligned bounding box. 1:1 with
/// `legacy::ray_intersects_aabb`.
#[expect(dead_code)]
pub fn ray_intersects_aabb(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> bool {
    let eps = 1e-6;
    let mut t_min = f32::NEG_INFINITY;
    let mut t_max = f32::INFINITY;

    let mut check_axis = |o: f32, d: f32, lo: f32, hi: f32| -> bool {
        if d.abs() < eps {
            return o >= lo && o <= hi;
        }
        let inv = 1.0 / d;
        let mut t1 = (lo - o) * inv;
        let mut t2 = (hi - o) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        t_min <= t_max
    };

    check_axis(origin.x, direction.x, min.x, max.x)
        && check_axis(origin.y, direction.y, min.y, max.y)
        && check_axis(origin.z, direction.z, min.z, max.z)
        && t_max >= 0.0
}

/// Project a cursor position (window-logical pixels) to a 2D world
/// position on the camera's look-at plane (z=0 in view space). The
/// output `Vec2` carries only the X and Y world coordinates — the
/// drag integrates `(new - last).extend(0.0)` into
/// `character_position`.
///
/// Returns `None` when the viewport is degenerate.
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
    let view = Mat4::look_at_rh(camera_eye, camera_target, camera_up);
    let world_3d = ene_vrm::view_pos_to_world(view_pos, view);
    Some(Vec2::new(world_3d.x, world_3d.y))
}

/// Process a press / release event. Mirrors
/// `legacy::update_drag_state`'s first half.
///
/// - `Pressed` + `cursor_over_character` → start drag, return
///   `DragAction::Started`. The current cursor world position is
///   stored as the drag origin (no movement is applied on the
///   press frame itself).
/// - `Pressed` + `!cursor_over_character` → no change.
/// - `Released` while dragging → clear state, return
///   `DragAction::Ended` so the runtime can call `mark_dirty()`.
/// - `Released` while not dragging → no change.
pub fn on_press_or_release(
    state: &mut CharacterDragState,
    event: DragButtonEvent,
    cursor_world_2d: Option<Vec2>,
    cursor_over_character: bool,
) -> DragAction {
    match event {
        DragButtonEvent::Pressed => {
            if cursor_over_character && let Some(p) = cursor_world_2d {
                state.last_cursor_world_pos = Some(p);
                return DragAction::Started;
            }
            DragAction::None
        }
        DragButtonEvent::Released => {
            if state.last_cursor_world_pos.is_some() {
                state.last_cursor_world_pos = None;
                return DragAction::Ended;
            }
            DragAction::None
        }
    }
}

/// Apply the per-frame drag delta. Mirrors `legacy::update_drag_state`'s
/// second half: when the user is dragging, integrate
/// `(cursor_world_pos - last_cursor_world_pos).extend(0.0)` into
/// `character_position` and update the stored origin.
///
/// Returns the delta to add to `character_position`, or `None` when
/// the cursor hasn't moved (or when the user isn't dragging).
pub fn tick(state: &mut CharacterDragState, cursor_world_2d: Option<Vec2>) -> Option<Vec3> {
    let last = state.last_cursor_world_pos?;
    let cur = cursor_world_2d?;
    if cur == last {
        return None;
    }
    state.last_cursor_world_pos = Some(cur);
    Some((cur - last).extend(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    const AABB_MIN: [f32; 3] = [-0.5, 0.0, -0.3];
    const AABB_MAX: [f32; 3] = [0.5, 1.4, 0.3];

    fn identity_model() -> Mat4 {
        Mat4::IDENTITY
    }

    fn translated_model(x: f32, y: f32, z: f32) -> Mat4 {
        Mat4::from_translation(Vec3::new(x, y, z))
    }

    fn scaled_model(scale: f32) -> Mat4 {
        Mat4::from_scale(Vec3::splat(scale))
    }

    #[test]
    fn aabb_world_corners_translation_moves_box() {
        let corners = aabb_world_corners(AABB_MIN, AABB_MAX, translated_model(1.0, 2.0, 0.0));
        let (min, max) = (
            corners
                .iter()
                .copied()
                .fold(Vec3::splat(f32::INFINITY), Vec3::min),
            corners
                .iter()
                .copied()
                .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max),
        );
        assert_eq!(min, Vec3::new(0.5, 2.0, -0.3));
        assert_eq!(max, Vec3::new(1.5, 3.4, 0.3));
    }

    #[test]
    fn aabb_world_corners_scale_grows_box() {
        let corners = aabb_world_corners(AABB_MIN, AABB_MAX, scaled_model(2.0));
        let (min, max) = (
            corners
                .iter()
                .copied()
                .fold(Vec3::splat(f32::INFINITY), Vec3::min),
            corners
                .iter()
                .copied()
                .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max),
        );
        assert_eq!(min, Vec3::new(-1.0, 0.0, -0.6));
        assert_eq!(max, Vec3::new(1.0, 2.8, 0.6));
    }

    #[test]
    fn transformed_aabb_bounds_identity_returns_local() {
        let (min, max) = transformed_aabb_bounds(AABB_MIN, AABB_MAX, identity_model());
        assert_eq!(min, Vec3::new(-0.5, 0.0, -0.3));
        assert_eq!(max, Vec3::new(0.5, 1.4, 0.3));
    }

    #[test]
    fn ray_intersects_aabb_hits_and_misses() {
        // Ray from (0, 1, 5) along -Z through the box at z=[-0.3, 0.3]
        assert!(ray_intersects_aabb(
            Vec3::new(0.0, 1.0, 5.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::from(AABB_MIN),
            Vec3::from(AABB_MAX),
        ));
        // Ray from (10, 1, 5) misses in X
        assert!(!ray_intersects_aabb(
            Vec3::new(10.0, 1.0, 5.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::from(AABB_MIN),
            Vec3::from(AABB_MAX),
        ));
        // Ray parallel to Y axis at X=10, passes the X slab
        assert!(!ray_intersects_aabb(
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::from(AABB_MIN),
            Vec3::from(AABB_MAX),
        ));
    }

    #[test]
    fn on_press_over_character_starts_drag() {
        let mut s = CharacterDragState::default();
        let action = on_press_or_release(
            &mut s,
            DragButtonEvent::Pressed,
            Some(Vec2::new(0.1, 0.2)),
            true,
        );
        assert_eq!(action, DragAction::Started);
        assert!(s.is_dragging());
        assert_eq!(s.last_cursor_world_pos, Some(Vec2::new(0.1, 0.2)));
    }

    #[test]
    fn on_press_outside_character_does_not_start() {
        let mut s = CharacterDragState::default();
        let action = on_press_or_release(
            &mut s,
            DragButtonEvent::Pressed,
            Some(Vec2::new(0.1, 0.2)),
            false,
        );
        assert_eq!(action, DragAction::None);
        assert!(!s.is_dragging());
    }

    #[test]
    fn on_press_without_cursor_pos_does_not_start() {
        let mut s = CharacterDragState::default();
        let action = on_press_or_release(&mut s, DragButtonEvent::Pressed, None, true);
        assert_eq!(action, DragAction::None);
        assert!(!s.is_dragging());
    }

    #[test]
    fn on_release_while_dragging_ends_and_signals() {
        let mut s = CharacterDragState {
            last_cursor_world_pos: Some(Vec2::new(0.0, 0.0)),
        };
        let action = on_press_or_release(&mut s, DragButtonEvent::Released, Some(Vec2::ZERO), true);
        assert_eq!(action, DragAction::Ended);
        assert!(!s.is_dragging());
    }

    #[test]
    fn on_release_while_idle_is_noop() {
        let mut s = CharacterDragState::default();
        let action = on_press_or_release(&mut s, DragButtonEvent::Released, Some(Vec2::ZERO), true);
        assert_eq!(action, DragAction::None);
    }

    #[test]
    fn tick_when_idle_is_none() {
        let mut s = CharacterDragState::default();
        assert_eq!(tick(&mut s, Some(Vec2::ZERO)), None);
    }

    #[test]
    fn tick_when_cursor_unchanged_is_none() {
        let mut s = CharacterDragState {
            last_cursor_world_pos: Some(Vec2::new(0.5, 0.5)),
        };
        assert_eq!(tick(&mut s, Some(Vec2::new(0.5, 0.5))), None);
        // Origin must not have been updated.
        assert_eq!(s.last_cursor_world_pos, Some(Vec2::new(0.5, 0.5)));
    }

    #[test]
    fn tick_when_cursor_moved_returns_delta_and_updates_origin() {
        let mut s = CharacterDragState {
            last_cursor_world_pos: Some(Vec2::new(0.0, 0.0)),
        };
        let delta = tick(&mut s, Some(Vec2::new(0.3, 0.2))).expect("delta");
        assert_eq!(delta, Vec3::new(0.3, 0.2, 0.0));
        assert_eq!(s.last_cursor_world_pos, Some(Vec2::new(0.3, 0.2)));
    }

    #[test]
    fn tick_with_no_cursor_while_dragging_ends_silently() {
        let mut s = CharacterDragState {
            last_cursor_world_pos: Some(Vec2::new(0.0, 0.0)),
        };
        assert_eq!(tick(&mut s, None), None);
        let again = tick(&mut s, None);
        assert_eq!(again, None);
    }

    #[test]
    fn cursor_logical_to_world_2d_at_viewport_center_returns_eye() {
        // For the ortho camera the cursor at the viewport center
        // projects onto the camera's local z=0 plane, which in
        // world space passes through the eye. This matches the
        // legacy `Camera::viewport_to_world_2d` semantics; the
        // drag system only cares about the *delta* between two
        // samples, not the absolute value.
        let world = cursor_logical_to_world_2d(
            Vec2::new(320.0, 240.0),
            (640, 480),
            Vec3::new(0.0, 0.3, 3.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        )
        .expect("projection");
        // The eye is at (0, 0.3, 3) and the view-z=0 plane passes
        // through it. Allow a tiny epsilon for the inverse
        // transform's float drift.
        assert!(world.x.abs() < 1e-3, "x = {}", world.x);
        assert!((world.y - 0.3).abs() < 1e-3, "y = {}", world.y);
    }

    #[test]
    fn cursor_logical_to_world_2d_delta_is_proportional_to_pixel_delta() {
        // The absolute position is the eye; what matters for the
        // drag integration is that the world delta is proportional
        // to the cursor pixel delta.
        let eye = Vec3::new(0.0, 0.3, 3.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let a = cursor_logical_to_world_2d(Vec2::new(100.0, 100.0), (640, 480), eye, target, up)
            .expect("a");
        let b = cursor_logical_to_world_2d(Vec2::new(110.0, 100.0), (640, 480), eye, target, up)
            .expect("b");
        let delta = b - a;
        // 10 pixels right at viewport_height=2.6 → 10 * (2.6 / 480) ≈ 0.0542 world units.
        assert!(delta.x > 0.0, "delta.x = {}", delta.x);
        assert!(delta.y.abs() < 1e-3, "delta.y = {}", delta.y);
        assert!(
            (delta.x - 10.0 * (2.6 / 480.0)).abs() < 1e-3,
            "delta.x = {} (expected ~{})",
            delta.x,
            10.0 * (2.6 / 480.0)
        );
    }

    #[test]
    fn cursor_logical_to_world_2d_degenerate_viewport_is_none() {
        let world = cursor_logical_to_world_2d(
            Vec2::ZERO,
            (0, 0),
            Vec3::new(0.0, 0.3, 3.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        );
        assert!(world.is_none());
    }
}
