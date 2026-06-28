//! Orthographic camera and per-frame [`CameraUniform`].
//!
//! Mirrors the legacy Bevy `MainViewCamera` in
//! `apps/ene-desktop/src/scene.rs`:
//! `ScalingMode::FixedVertical { viewport_height: 2.6 }`,
//! position `(0, 1, 3)` looking at `(0, 1, 0)`.
//!
//! A perspective projection can be added later without touching the
//! public API; only the [`CameraUniform::view_proj`] matrix needs to
//! change.
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use crate::error::VrmResult;

/// Viewport height in world units (matches the legacy
/// `ScalingMode::FixedVertical { viewport_height: 2.6 }`).
pub const VIEWPORT_HEIGHT: f32 = 2.6;

/// Default camera position.
pub const DEFAULT_EYE: [f32; 3] = [0.0, 0.3, 3.0];

/// Default look-at target.
pub const DEFAULT_TARGET: [f32; 3] = [0.0, 0.0, 0.0];

/// Up vector.
pub const DEFAULT_UP: [f32; 3] = [0.0, 1.0, 0.0];

/// CPU-side orthographic camera that builds a per-frame
/// [`CameraUniform`].
#[derive(Debug, Clone)]
pub struct OrthographicCamera {
    eye: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    viewport_height: f32,
    /// Aspect ratio = width / height. Updated by the renderer every
    /// frame from the surface size.
    aspect: f32,
}

impl Default for OrthographicCamera {
    fn default() -> Self {
        Self {
            eye: DEFAULT_EYE,
            target: DEFAULT_TARGET,
            up: DEFAULT_UP,
            viewport_height: VIEWPORT_HEIGHT,
            aspect: 1.0,
        }
    }
}

impl OrthographicCamera {
    /// Set the aspect ratio (width / height). Called every frame by
    /// the renderer from the wgpu surface size.
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect.max(0.0001);
    }

    /// Update the camera eye and target. The runtime wires
    /// these to `CharacterState::character_position` and the
    /// LookAt target.
    pub fn look_at(&mut self, eye: [f32; 3], target: [f32; 3]) {
        self.eye = eye;
        self.target = target;
    }

    /// Returns the current camera eye position.
    pub fn eye(&self) -> [f32; 3] {
        self.eye
    }

    /// Returns the current camera target position.
    pub fn target(&self) -> [f32; 3] {
        self.target
    }

    /// Diagnostic: returns `(eye, target, viewport_height, aspect)`.
    pub fn debug(&self) -> ([f32; 3], [f32; 3], f32, f32) {
        (self.eye, self.target, self.viewport_height, self.aspect)
    }

    /// Compute the scale factor that makes an axis-aligned bounding
    /// box of the given extents fit the current orthographic
    /// viewport, leaving `margin` of the viewport unused on every
    /// side. `margin = 0.9` is a sensible default (5 % padding on
    /// each side).
    ///
    /// Used by `ene-desktop` to keep user-supplied
    /// `model_scale` values from rendering the model larger than
    /// the viewport. The runtime multiplies the returned scale by
    /// the user's slider value, so `model_scale = 1.0` is "the
    /// model fits the viewport" rather than "1× whatever the
    /// loader happened to normalise to".
    pub fn compute_auto_fit_scale(
        &self,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
        margin: f32,
    ) -> f32 {
        let extent_x = (aabb_max[0] - aabb_min[0]).max(0.0001);
        let extent_y = (aabb_max[1] - aabb_min[1]).max(0.0001);
        let viewport_w = self.viewport_height * self.aspect;
        let scale_x = viewport_w * margin / extent_x;
        let scale_y = self.viewport_height * margin / extent_y;
        scale_x.min(scale_y).max(0.0001)
    }

    /// Build the per-frame uniform.
    pub fn uniform(&self) -> VrmResult<CameraUniform> {
        let view = self.debug_view();
        let half_h = self.viewport_height * 0.5;
        let half_w = half_h * self.aspect;
        // glam's `orthographic` expects `near` and `far` to be
        // **positive** distances from the camera plane. A negative
        // `near` shifts the depth range so that geometry between
        // the camera and the original near plane is clipped —
        // making the model look like a tiny silhouette.
        let proj = glam::camera::rh::proj::directx::orthographic(
            -half_w, half_w, -half_h, half_h, 0.1, 100.0,
        );
        Ok(CameraUniform {
            view_proj: (proj * view).to_cols_array_2d(),
            camera_pos: [self.eye[0], self.eye[1], self.eye[2], 1.0],
        })
    }

    /// Diagnostic: just the view matrix (the look_at
    /// rotation+translation). Exposed so the runtime can dump
    /// it without having to plumb the private eye/target/up
    /// fields out to the desktop app.
    pub fn debug_view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye.into(), self.target.into(), self.up.into())
    }

    /// Diagnostic: just the orthographic projection
    /// matrix. Used by `runtime.rs` to verify the projection
    /// side isn't the source of a height-related bug.
    pub fn debug_proj(&self) -> Mat4 {
        let half_h = self.viewport_height * 0.5;
        let half_w = half_h * self.aspect;
        glam::camera::rh::proj::directx::orthographic(-half_w, half_w, -half_h, half_h, 0.1, 100.0)
    }
}

/// Uniform buffer data uploaded each frame.
///
/// Total size: 80 bytes (16-byte aligned: 64 for `view_proj` + 16
/// for `camera_pos`). All VRM shaders (lite / skinned / unlit /
/// MToon) share this layout so the same `camera` bind group can
/// be bound on every pipeline.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CameraUniform {
    /// Combined view-projection matrix in column-major form.
    pub view_proj: [[f32; 4]; 4],
    /// Camera world-space position with `w = 1.0`. The MToon
    /// shader uses this to build the view direction for matcap /
    /// fresnel rim.
    pub camera_pos: [f32; 4],
}

/// Per-frame model transform uniform. The runtime composes a
/// translation and uniform scale from `CharacterState` and
/// uploads it as a single `mat4x4`. The vertex shader applies
/// `view_proj * model * pos`; a per-joint palette is multiplied
/// on top of this (skin) and a humanoid root rotation (model
/// rotation).
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct ModelUniform {
    /// Combined model-to-world matrix in column-major form.
    pub model: [[f32; 4]; 4],
}

impl Default for ModelUniform {
    fn default() -> Self {
        Self {
            model: Mat4::IDENTITY.to_cols_array_2d(),
        }
    }
}

impl ModelUniform {
    /// Build a model matrix from a translation (world units) and a
    /// uniform scale.
    ///
    /// No rotation is applied: VRoid (Alicia) and other VRM 1.0
    /// humanoid models are exported with their **face already at
    /// `+Z`** (the legacy `apps/ene-desktop` Bevy build used
    /// `Transform::from_translation(character_position).with_scale(model_scale)`
    /// with no extra rotation and was rendering the character
    /// front-facing toward the camera at `+Z`). With culling on
    /// (`CullMode::Back`, `FrontFace::Ccw`) the camera at `+Z` sees
    /// the model as front-facing, so it is kept. An earlier
    /// 180°-around-Y pre-rotation was the wrong direction; it
    /// showed the back of the character and mirrored `character_state.character_position.x`,
    /// which was what was making the model appear shifted to the
    /// right and half off-screen.
    pub fn from_position_scale(position: [f32; 3], scale: f32) -> Self {
        let m = Mat4::from_scale_rotation_translation(
            glam::Vec3::splat(scale),
            glam::Quat::IDENTITY,
            position.into(),
        );
        Self {
            model: m.to_cols_array_2d(),
        }
    }

    /// Wrap an already-composed `Mat4` (e.g. one built by
    /// `CharacterRenderer::model_matrix` that folds the
    /// loader's `T(-center) * S(normalize_scale)` in alongside
    /// the character's translation and per-frame scale).
    /// Kept as a thin conversion so the renderer and the
    /// runtime can compose the matrix in `glam` space and
    /// hand the result off in one shot.
    pub fn from_mat4(m: Mat4) -> Self {
        Self {
            model: m.to_cols_array_2d(),
        }
    }
}

/// Minimum `aspect` value used to avoid a divide-by-zero when the
/// viewport collapses to a single axis. Matches the
/// `aspect.max(0.0001)` guard used by every caller of the
/// `ndc_to_view_pos*` helpers.
const MIN_ASPECT: f32 = 0.0001;

/// Convert a window-pixel coordinate to normalised device
/// coordinates (Y flipped to match the NDC convention, since
/// winit's cursor origin is top-left and NDC's is bottom-left).
pub fn pixel_to_ndc(px: f32, py: f32, viewport: (u32, u32)) -> Vec2 {
    let w = viewport.0.max(1) as f32;
    let h = viewport.1.max(1) as f32;
    Vec2::new((px / w) * 2.0 - 1.0, -((py / h) * 2.0 - 1.0))
}

/// Convert NDC to a view-space position on the orthographic
/// camera's near plane, deriving the aspect from `viewport`.
/// Use this overload when you have the viewport but not the
/// aspect pre-computed.
pub fn ndc_to_view_pos(ndc: Vec2, viewport: (u32, u32), view_z: f32) -> Vec3 {
    let aspect = (viewport.0.max(1) as f32 / viewport.1.max(1) as f32).max(MIN_ASPECT);
    ndc_to_view_pos_with_aspect(ndc, aspect, view_z)
}

/// Convert NDC to a view-space position on the orthographic
/// camera's near plane, using a pre-computed `aspect`. The
/// `view_z` parameter lets the caller choose where along the
/// view axis to land (drag uses `0.0`, look-at uses
/// `head_view.z + NEUTRAL_TARGET_Z`, debug overlays use a
/// fixed `-3.0` so the wireframe sits in front of the model).
pub fn ndc_to_view_pos_with_aspect(ndc: Vec2, aspect: f32, view_z: f32) -> Vec3 {
    let aspect = aspect.max(MIN_ASPECT);
    let half_h = VIEWPORT_HEIGHT * 0.5;
    let half_w = half_h * aspect;
    Vec3::new(ndc.x * half_w, ndc.y * half_h, view_z)
}

/// Inverse-transform a view-space point back to world space
/// using the camera's `view` matrix. Convenience wrapper around
/// `view.inverse().transform_point3(p)` so callers do not have
/// to think about the inverse.
pub fn view_pos_to_world(view_pos: Vec3, view: Mat4) -> Vec3 {
    view.inverse().transform_point3(view_pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(aspect: f32) -> OrthographicCamera {
        let mut c = OrthographicCamera::default();
        c.set_aspect(aspect);
        c
    }

    /// A 1.5 × 1.5 box at viewport (2.6h × 3.9w, aspect 1.5) is
    /// Y-limited: the longest axis is Y, so `scale_y` drives the
    /// fit. `scale_y = 2.6 * 0.9 / 1.5 = 1.56`. `scale_x` would be
    /// `3.9 * 0.9 / 1.5 = 2.34`, larger, so `min` picks 1.56.
    #[test]
    fn auto_fit_tall_box_at_1_5_aspect_picks_y_axis() {
        let c = camera(1.5);
        let s = c.compute_auto_fit_scale([-0.75, 0.0, 0.0], [0.75, 1.5, 0.0], 0.9);
        assert!((s - 1.56).abs() < 1e-4, "expected ~1.56, got {s}");
    }

    /// A 2.0 × 1.0 box at the same viewport is X-limited: `scale_x
    /// = 3.9 * 0.9 / 2.0 = 1.755`, `scale_y = 2.6 * 0.9 / 1.0 =
    /// 2.34`. `min` picks 1.755.
    #[test]
    fn auto_fit_wide_box_at_1_5_aspect_picks_x_axis() {
        let c = camera(1.5);
        let s = c.compute_auto_fit_scale([-1.0, 0.0, 0.0], [1.0, 1.0, 0.0], 0.9);
        assert!((s - 1.755).abs() < 1e-4, "expected ~1.755, got {s}");
    }

    /// A zero-extent AABB (degenerate / uninitialised model) must
    /// not produce NaN / Inf. The implementation clamps the divisor
    /// to `0.0001` and the result to a `max(0.0001)` floor.
    #[test]
    fn auto_fit_degenerate_aabb_returns_floor() {
        let c = camera(1.5);
        let s = c.compute_auto_fit_scale([0.0; 3], [0.0; 3], 0.9);
        assert!(s.is_finite(), "expected finite scale, got {s}");
        assert!(s >= 0.0001, "expected >= 0.0001, got {s}");
    }

    /// Margin of 1.0 (no padding) on a 1 × 1 box at aspect 1.5
    /// should produce exactly `2.6 * 1.0 / 1.0 = 2.6` (Y-driven).
    #[test]
    fn auto_fit_margin_one_yields_exact_viewport_height() {
        let c = camera(1.5);
        let s = c.compute_auto_fit_scale([0.0, 0.0, 0.0], [1.0, 1.0, 0.0], 1.0);
        assert!((s - 2.6).abs() < 1e-4, "expected 2.6, got {s}");
    }

    /// diagnostic: confirm what glam's
    /// `orthographic(-half_w, half_w, -half_h, half_h, 0.1, 100.0)`
    /// actually produces, so the-diag log can be
    /// interpreted correctly. Just prints the matrix.
    #[test]
    fn orthographic_rh_diag_640x480() {
        let half_h = 2.6 * 0.5;
        let half_w = half_h * 1.3333333;
        let p = glam::camera::rh::proj::directx::orthographic(
            -half_w, half_w, -half_h, half_h, 0.1, 100.0,
        );
        println!("P = {p:?}");
    }

    /// `pixel_to_ndc` centres NDC around the window and flips Y
    /// (winit's cursor origin is top-left, NDC's is bottom-left).
    #[test]
    fn pixel_to_ndc_centres_and_flips_y() {
        let ndc = pixel_to_ndc(320.0, 240.0, (640, 480));
        assert!((ndc.x - 0.0).abs() < 1e-6);
        assert!((ndc.y - 0.0).abs() < 1e-6);
        let ndc_corner = pixel_to_ndc(0.0, 0.0, (640, 480));
        assert!((ndc_corner.x - -1.0).abs() < 1e-6);
        assert!((ndc_corner.y - 1.0).abs() < 1e-6);
    }

    /// `ndc_to_view_pos_with_aspect` scales NDC by half the
    /// viewport extents and lands at the requested view_z.
    #[test]
    fn ndc_to_view_pos_scales_by_viewport_extents() {
        let v = ndc_to_view_pos_with_aspect(Vec2::new(1.0, 1.0), 1.5, -3.0);
        let half_h = VIEWPORT_HEIGHT * 0.5;
        let half_w = half_h * 1.5;
        assert!((v.x - half_w).abs() < 1e-6);
        assert!((v.y - half_h).abs() < 1e-6);
        assert!((v.z - -3.0).abs() < 1e-6);
    }

    /// `ndc_to_view_pos` (viewport-based) must agree with the
    /// aspect-based variant.
    #[test]
    fn ndc_to_view_pos_viewport_agrees_with_aspect() {
        let v_via_viewport = ndc_to_view_pos(Vec2::new(0.5, -0.5), (800, 400), 0.0);
        let v_via_aspect = ndc_to_view_pos_with_aspect(Vec2::new(0.5, -0.5), 2.0, 0.0);
        assert!((v_via_viewport.x - v_via_aspect.x).abs() < 1e-6);
        assert!((v_via_viewport.y - v_via_aspect.y).abs() < 1e-6);
        assert!((v_via_viewport.z - 0.0).abs() < 1e-6);
    }

    /// `view_pos_to_world` is the inverse of `view.transform_point3`.
    #[test]
    fn view_pos_to_world_round_trips_through_view() {
        let eye = Vec3::new(0.0, 0.3, 3.0);
        let target = Vec3::new(0.0, 0.0, 0.0);
        let up = Vec3::new(0.0, 1.0, 0.0);
        let view = Mat4::look_at_rh(eye, target, up);
        let world_point = Vec3::new(1.2, 0.5, -0.4);
        let view_point = view.transform_point3(world_point);
        let back = view_pos_to_world(view_point, view);
        assert!((back.x - world_point.x).abs() < 1e-5);
        assert!((back.y - world_point.y).abs() < 1e-5);
        assert!((back.z - world_point.z).abs() < 1e-5);
    }
}
