//! Cursor → smoothed world target for the head-look-at system.
//!
//! PR4.2: port of the legacy
//! `apps/ene-desktop/src/character.rs::update_cursor_look_target`
//! (lines 435–478) and `body_tracking_for_strength` (lines 514–537).
//! The smoothed world target is stored in [`LookAtState`]; the
//! runtime exposes it to the renderer. PR4.5+ (skinning) will use
//! it to drive humanoid bone rotations. For now PR4.2 only stores
//! the value and feeds it to the orthographic camera's eye
//! position so a subtle pan of the camera tracks the cursor —
//! the visible "look at" effect itself waits for skinning.
//!
//! All numbers match the legacy exactly so screenshots /
//! regression tests stay comparable:
//!
//! - smoothing speed `7.0`,
//! - neutral target `(head_x, head_y, head_z + 1.8)`,
//! - cursor NDC multiplied by `viewport_height / 2` and
//!   `viewport_height / 2 * aspect`,
//! - ray intersected with the plane through the head, normal
//!   `camera_forward` (world-space).
use glam::{Mat4, Vec2, Vec3};

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

const SMOOTHING_SPEED: f32 = 7.0;

/// Z offset (in world units) of the neutral "look straight ahead"
/// target relative to the head. Mirrors the legacy `Vec3::new(0, 0,
/// 1.8)`.
const NEUTRAL_TARGET_Z: f32 = 1.8;

/// Convert a cursor position in window-logical pixels to normalized
/// device coordinates, with the Y axis flipped (winit's origin is
/// top-left, NDC's origin is bottom-left).
pub fn cursor_logical_to_ndc(cursor: Vec2, viewport: (u32, u32)) -> Vec2 {
    let w = viewport.0.max(1) as f32;
    let h = viewport.1.max(1) as f32;
    Vec2::new((cursor.x / w) * 2.0 - 1.0, -((cursor.y / h) * 2.0 - 1.0))
}

/// Neutral head-look target (straight ahead of the head).
pub fn neutral_target(head_world: Vec3) -> Vec3 {
    head_world + Vec3::new(0.0, 0.0, NEUTRAL_TARGET_Z)
}

/// Compute the smoothed world target the character should look at.
///
/// `state` is updated in place. Returns the new smoothed target.
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
) -> Vec3 {
    let ndc = cursor_logical_to_ndc(cursor_logical, viewport_size);
    let aspect = (viewport_size.0 as f32 / viewport_size.1 as f32).max(0.0001);
    let half_h = ene_vrm::camera::VIEWPORT_HEIGHT * 0.5;
    let half_w = half_h * aspect;
    // Cursor position in view space, at z=0 (the camera's look-at
    // plane).
    let view_pos = Vec3::new(ndc.x * half_w, ndc.y * half_h, 0.0);

    let view = Mat4::look_at_rh(camera_eye, camera_target, camera_up);
    let world_at_view_z0 = view.inverse().transform_point3(view_pos);

    let camera_forward = (camera_target - camera_eye).normalize_or_zero();
    let ray_dir = (world_at_view_z0 - camera_eye).normalize_or_zero();

    let head_to_eye = camera_eye - head_world;
    let denom = ray_dir.dot(camera_forward);
    let t = if denom.abs() < 1e-6 {
        0.0
    } else {
        head_to_eye.dot(camera_forward) / denom
    };
    let cursor_world = camera_eye + ray_dir * t;

    let strength = strength.clamp(0.0, 1.0);
    let neutral = neutral_target(head_world);
    let desired = neutral.lerp(cursor_world, strength);

    let smoothed = if let Some(current) = state.smoothed_world_target {
        let alpha = 1.0 - (-SMOOTHING_SPEED * dt_secs).exp();
        current.lerp(desired, alpha)
    } else {
        desired
    };

    state.smoothed_world_target = Some(smoothed);
    state.last_cursor_logical = Some(cursor_logical);
    smoothed
}

/// Per-bone body-tracking weights and limits, derived from the
/// `look_at_strength` slider. Mirrors the legacy
/// `body_tracking_for_strength` 1:1.
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
