//! Cursor → smoothed world target for the head-look-at system.
//!
//! The smoothed world target is stored in [`LookAtState`]; the
//! runtime exposes it to the renderer. Skinning (future work) will
//! use it to drive humanoid bone rotations; for now the value is
//! fed to the orthographic camera so a subtle pan tracks the cursor.
//!
//! All numbers match the legacy exactly so screenshots and
//! regression tests stay comparable: smoothing speed `7.0`,
//! neutral target `(head_x, head_y, head_z + 1.8)`, cursor NDC
//! scaled by `viewport_height / 2` and `viewport_height / 2 *
//! aspect`, ray intersected with the plane through the head with
//! normal `camera_forward` (world space).
use glam::{Vec2, Vec3};

/// Look-at state. Owned by `CharacterRenderer` so it survives
/// across frames.
#[derive(Debug, Default, Clone)]
pub struct LookAtState {
    /// Smoothed world target. `None` until the first cursor sample
    /// is processed.
    pub smoothed_world_target: Option<Vec3>,
    /// Last cursor position in window-logical pixels. Mirrored from
    /// `winit::event::CursorMoved::position` for debugging.
    pub last_cursor_logical: Option<Vec2>,
}

/// Z offset (in world units) of the neutral "look straight ahead"
/// target relative to the head.
const NEUTRAL_TARGET_Z: f32 = 1.8;

/// Y offset (in world units) of the head above the character's
/// origin. An approximation of a humanoid head position above the
/// model's pivot. Acts as the fallback for models without a
/// humanoid `head` bone; those use the bone's rest position scaled
/// by `model_scale` instead.
pub const HEAD_OFFSET_Y: f32 = 1.0;

/// Build the world-space head position from a model's pivot.
pub fn head_world_for(pivot: Vec3) -> Vec3 {
    pivot + Vec3::new(0.0, HEAD_OFFSET_Y, 0.0)
}

/// Convert a cursor position in window-logical pixels to normalized
/// device coordinates, with the Y axis flipped (winit's origin is
/// top-left, NDC's origin is bottom-left). Thin re-export of
/// [`ene_vrm::pixel_to_ndc`] so internal callers can keep using the
/// `cursor_logical_to_ndc` name.
pub fn cursor_logical_to_ndc(cursor: Vec2, viewport: (u32, u32)) -> Vec2 {
    ene_vrm::pixel_to_ndc(cursor.x, cursor.y, viewport)
}

/// Neutral head-look target (straight ahead of the head).
pub fn neutral_target(head_world: Vec3) -> Vec3 {
    head_world + Vec3::new(0.0, 0.0, NEUTRAL_TARGET_Z)
}

/// Compute the smoothed world target the character should look at.
///
/// `state` is updated in place. Returns the new smoothed target.
///
/// `smoothing` is the per-frame exponential-smoothing rate (in
/// `1/seconds`). The VRM 1.0 spec does not declare a smoothing
/// value, so callers pass
/// [`ene_vrm::LookAtProperties::DEFAULT_SMOOTHING`] by default.
#[allow(clippy::too_many_arguments)]
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
    smoothing: f32,
) -> Vec3 {
    let ndc = cursor_logical_to_ndc(cursor_logical, viewport_size);
    let view = glam::camera::rh::view::look_at_mat4(camera_eye, camera_target, camera_up);
    let head_view = view.transform_point3(head_world);
    let aspect = (viewport_size.0 as f32 / viewport_size.1 as f32).max(0.0001);
    let view_pos =
        ene_vrm::ndc_to_view_pos_with_aspect(ndc, aspect, head_view.z + NEUTRAL_TARGET_Z);
    let cursor_world = ene_vrm::view_pos_to_world(view_pos, view);

    let strength = strength.clamp(0.0, 1.0);
    let neutral = neutral_target(head_world);
    let desired = neutral.lerp(cursor_world, strength);

    let smoothing = if smoothing > 0.0 { smoothing } else { 0.0 };
    let smoothed = if let Some(current) = state.smoothed_world_target {
        let alpha = 1.0 - (-smoothing * dt_secs).exp();
        current.lerp(desired, alpha)
    } else {
        desired
    };

    state.smoothed_world_target = Some(smoothed);
    state.last_cursor_logical = Some(cursor_logical);
    smoothed
}

/// Per-bone body-tracking weights and limits, derived from the
/// `look_at_strength` slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyTracking {
    pub head_weight: f32,
    pub neck_weight: f32,
    pub chest_weight: f32,
    pub spine_weight: f32,

    pub head_yaw_max: f32,
    pub head_pitch_max: f32,
    pub neck_yaw_max: f32,
    pub neck_pitch_max: f32,
    pub chest_yaw_max: f32,
    pub chest_pitch_max: f32,
    pub spine_yaw_max: f32,
    pub spine_pitch_max: f32,

    pub smoothing: f32,
    pub output_smoothing: f32,
    pub reference_depth: f32,
}

pub fn body_tracking_for_strength(strength: f32) -> BodyTracking {
    let s = strength.clamp(0.0, 1.0);
    let influence = s;
    let angle_scale = 0.20 + 0.80 * s;
    BodyTracking {
        head_weight: 0.24 * influence,
        neck_weight: 0.14 * influence,
        chest_weight: 0.08 * influence,
        spine_weight: 0.04 * influence,

        head_yaw_max: 22.0 * angle_scale,
        head_pitch_max: 16.0 * angle_scale,
        neck_yaw_max: 14.0 * angle_scale,
        neck_pitch_max: 10.0 * angle_scale,
        chest_yaw_max: 8.0 * angle_scale,
        chest_pitch_max: 4.0 * angle_scale,
        spine_yaw_max: 5.0 * angle_scale,
        spine_pitch_max: 2.0 * angle_scale,

        smoothing: 4.5 + 5.5 * s,
        output_smoothing: 6.5 + 7.5 * s,
        reference_depth: (1.20 + (1.0 - s) * 0.45).clamp(1.0, 1.8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_world_for_offsets_y_only() {
        let pivot = Vec3::new(-1.5, 0.0, 2.25);
        let head = head_world_for(pivot);
        assert_eq!(head.x, pivot.x);
        assert_eq!(head.z, pivot.z);
        assert_eq!(head.y, HEAD_OFFSET_Y);
    }

    #[test]
    fn head_offset_y_constant_is_documented() {
        assert_eq!(HEAD_OFFSET_Y, 1.0);
    }
}
