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
