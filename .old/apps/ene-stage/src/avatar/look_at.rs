//! Cursor → smoothed world target for avatar head look-at.

use glam::{Vec2, Vec3};

use ene_vrm::LookAtProperties;

const NEUTRAL_TARGET_Z: f32 = 1.8;

/// Smoothed look-at target carried across frames.
#[derive(Debug, Default)]
pub struct LookAtState {
    pub smoothed_world_target: Option<Vec3>,
}

pub fn neutral_target(head_world: Vec3) -> Vec3 {
    head_world + Vec3::new(0.0, 0.0, NEUTRAL_TARGET_Z)
}

pub fn compute_world_target(
    cursor_logical: Vec2,
    viewport_size: (u32, u32),
    camera_eye: Vec3,
    camera_target: Vec3,
    camera_up: Vec3,
    head_world: Vec3,
    strength: f32,
    state: &mut LookAtState,
    dt_secs: f32,
) -> Vec3 {
    let ndc = ene_vrm::pixel_to_ndc(cursor_logical.x, cursor_logical.y, viewport_size);
    let view = glam::camera::rh::view::look_at_mat4(camera_eye, camera_target, camera_up);
    let head_view = view.transform_point3(head_world);
    let aspect = (viewport_size.0 as f32 / viewport_size.1 as f32).max(0.0001);
    let view_pos =
        ene_vrm::ndc_to_view_pos_with_aspect(ndc, aspect, head_view.z + NEUTRAL_TARGET_Z);
    let cursor_world = ene_vrm::view_pos_to_world(view_pos, view);

    let strength = strength.clamp(0.0, 1.0);
    let neutral = neutral_target(head_world);
    let desired = neutral.lerp(cursor_world, strength);

    let smoothing = LookAtProperties::DEFAULT_SMOOTHING;
    let smoothed = if let Some(current) = state.smoothed_world_target {
        let alpha = 1.0 - (-smoothing * dt_secs).exp();
        current.lerp(desired, alpha)
    } else {
        desired
    };

    state.smoothed_world_target = Some(smoothed);
    smoothed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> (Vec3, Vec3, Vec3, Vec3) {
        (
            Vec3::new(0.0, 1.0, 4.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::Y,
            Vec3::new(0.0, 1.2, 0.0),
        )
    }

    #[test]
    fn neutral_target_is_in_front_of_the_head() {
        let head = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(neutral_target(head), head + Vec3::new(0.0, 0.0, 1.8));
    }

    #[test]
    fn strength_zero_stays_on_the_neutral_axis() {
        let (eye, target, up, head) = camera();
        let mut state = LookAtState::default();
        let world = compute_world_target(
            Vec2::new(100.0, 50.0),
            (200, 100),
            eye,
            target,
            up,
            head,
            0.0,
            &mut state,
            1.0,
        );
        let expected = neutral_target(head);
        assert!(world.distance(expected) < 1e-4);
    }

    #[test]
    fn strength_one_follows_the_cursor_and_zero_dt_does_not_smooth() {
        let (eye, target, up, head) = camera();
        let mut left_state = LookAtState::default();
        let mut right_state = LookAtState::default();
        let left = compute_world_target(
            Vec2::new(0.0, 50.0),
            (200, 100),
            eye,
            target,
            up,
            head,
            1.0,
            &mut left_state,
            1.0,
        );
        let right = compute_world_target(
            Vec2::new(200.0, 50.0),
            (200, 100),
            eye,
            target,
            up,
            head,
            1.0,
            &mut right_state,
            1.0,
        );
        assert!(
            left.distance(right) > 0.01,
            "left and right cursor must produce different look-at targets"
        );

        let mut held_state = LookAtState {
            smoothed_world_target: Some(left),
        };
        let held_again = compute_world_target(
            Vec2::new(200.0, 50.0),
            (200, 100),
            eye,
            target,
            up,
            head,
            1.0,
            &mut held_state,
            0.0,
        );
        assert!(
            held_again.distance(left) < 1e-5,
            "dt=0 must not advance smoothing"
        );
    }

    #[test]
    fn zero_viewport_does_not_panic() {
        let (eye, target, up, head) = camera();
        let mut state = LookAtState::default();
        let world = compute_world_target(
            Vec2::ZERO,
            (0, 0),
            eye,
            target,
            up,
            head,
            0.5,
            &mut state,
            0.016,
        );
        assert!(world.is_finite());
    }
}
