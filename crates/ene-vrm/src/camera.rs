//! Orthographic camera and per-frame [`CameraUniform`].
//!
//! PR3 ships only an orthographic projection, mirroring the legacy
//! Bevy `MainViewCamera` in `apps/ene-desktop/src/scene.rs`:
//! `ScalingMode::FixedVertical { viewport_height: 2.6 }`,
//! position `(0, 1, 3)` looking at `(0, 1, 0)`.
//!
//! A perspective projection can be added later without touching the
//! public API; only the [`CameraUniform::view_proj`] matrix needs to
//! change.
use bytemuck::{Pod, Zeroable};
use glam::Mat4;

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

    /// Update the camera eye / target (PR4 will wire these to the
    /// `CharacterState::character_position` and the LookAt target).
    pub fn look_at(&mut self, eye: [f32; 3], target: [f32; 3]) {
        self.eye = eye;
        self.target = target;
    }

    /// Build the per-frame uniform.
    pub fn uniform(&self) -> VrmResult<CameraUniform> {
        let view = Mat4::look_at_rh(self.eye.into(), self.target.into(), self.up.into());
        let half_h = self.viewport_height * 0.5;
        let half_w = half_h * self.aspect;
        // glam's `orthographic_rh` expects `near` and `far` to be
        // **positive** distances from the camera plane. A negative
        // `near` (the previous value was `-10.0`) shifts the depth
        // range so that geometry between the camera and the
        // original near plane is clipped — which is exactly what
        // was making the model look like a tiny silhouette.
        let proj = Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, 0.1, 100.0);
        Ok(CameraUniform {
            view_proj: (proj * view).to_cols_array_2d(),
        })
    }
}

/// Uniform buffer data uploaded each frame.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct CameraUniform {
    /// Combined view-projection matrix in column-major form.
    pub view_proj: [[f32; 4]; 4],
}

/// Per-frame model transform uniform. PR4.1: the runtime
/// composes a translation (`position`) and uniform scale from
/// `CharacterState` and uploads it as a single `mat4x4`. The
/// vertex shader applies `view_proj * model * pos`. PR4.x will
/// add a per-joint palette on top of this (skin) and a humanoid
/// root rotation (model rotation).
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
    /// which is what was making the model appear shifted to the
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
}
