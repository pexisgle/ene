//! Character components.
//!
//! Each component is a small plain-data wrapper around one piece of
//! the legacy [`crate::character::CharacterRenderer`]. Adding a
//! component to an entity gives the systems the data they need; the
//! legacy struct becomes a thin read-through cache during the
//! migration.
use std::path::PathBuf;
use std::sync::Arc;

use bevy_ecs::prelude::*;
use glam::{Quat, Vec3};

use crate::character::collider::BoneShapeSpec;
use crate::character_state::ActiveEmotion;
use ene_vrm::{
    OrthographicCamera, SpringBoneSimulator, VrmModel, VrmRenderer, VrmaAsset, VrmaPlayer,
};

/// Marker for the primary character entity. Added in addition to the
/// data components so systems can query for "the character" without
/// relying on entity id.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CharacterRoot;

/// Loaded VRM model shared with the GPU renderer. The handle is
/// `Arc`-backed so the renderer and the systems share the same
/// underlying mesh / skinning data without copying.
#[derive(Component, Default)]
pub struct VrmModelHandle {
    /// `None` when the default VRM is missing; systems should treat
    /// this as "render nothing" and only clear the surface.
    pub model: Option<VrmModel>,
    /// Initialised by `CharacterPlugin::finish` once a GPU device
    /// exists. The legacy code currently constructs the renderer
    /// inside `CharacterRenderer::init`; Phase 7 moves that to a
    /// `Last`-stage system using `NonSend<wgpu::Device>`.
    pub renderer: Option<VrmRenderer>,
}

impl std::fmt::Debug for VrmModelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VrmModelHandle")
            .field("model_loaded", &self.model.is_some())
            .field("renderer_built", &self.renderer.is_some())
            .finish()
    }
}

/// Currently-loaded motion asset. `None` if no motion has been
/// triggered yet.
#[derive(Component, Default)]
pub struct MotionState {
    /// Loaded `.vrma` bytes. Re-loaded by `play_motion`.
    pub asset: Option<Arc<VrmaAsset>>,
    /// Playback machine.
    #[expect(dead_code, reason = "Read by motion system in Phase 3+")]
    pub player: VrmaPlayer,
    /// Path of the current motion file. `None` until first
    /// `play_motion` call.
    pub path: Option<PathBuf>,
}

impl std::fmt::Debug for MotionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MotionState")
            .field("asset_loaded", &self.asset.is_some())
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Spring-bone simulator + cached properties. `None` for models
/// without `VRMC_springBone`.
#[derive(Component, Default)]
pub struct SpringBoneState(pub Option<SpringBoneSimulator>);

impl std::fmt::Debug for SpringBoneState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpringBoneState")
            .field("has_simulator", &self.0.is_some())
            .finish()
    }
}

/// Orthographic camera for the character window. The fields match
/// [`OrthographicCamera`] so the legacy `camera_eye` / `camera_target`
/// helpers can be implemented as trivial conversions.
#[derive(Component, Debug, Clone, Default)]
pub struct CharacterCamera(
    #[expect(dead_code, reason = "Read by render system in Phase 7")] pub OrthographicCamera,
);

/// Per-character look-at state. `strength = 0` disables the effect.
#[derive(Component, Debug, Default, Clone)]
pub struct LookAt {
    /// User-configured strength (0–1). `0` means "no look-at".
    #[expect(dead_code, reason = "Read by look-at system in Phase 3+")]
    pub strength: f32,
    /// Smoothed world-space target. Updated each frame by
    /// `update_look_at` (Phase 3+ system).
    #[expect(dead_code, reason = "Read by look-at system in Phase 3+")]
    pub smoothed_world_target: Vec3,
    /// Last known logical cursor position.
    #[expect(dead_code, reason = "Read by look-at system in Phase 3+")]
    pub last_cursor_logical: Option<Vec2Wrapper>,
}

/// Newtype to avoid pulling `glam::Vec2` into a public component
/// field that is otherwise plain-data.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Vec2Wrapper(pub Vec3);

/// Emotion channel: pending queue + active emotion.
#[derive(Component, Debug, Default)]
pub struct EmotionChannel {
    /// Currently-applied emotion, if any.
    #[expect(dead_code, reason = "Read by emotion system in Phase 3+")]
    pub active: Option<ActiveEmotion>,
}

/// Per-character bone collider spec. Populated by
/// `build_character_bone_specs` and consumed by the physics
/// plugin (Phase 4).
#[derive(Component, Debug, Default, Clone)]
pub struct BoneColliders(pub Vec<BoneShapeSpec>);

/// Position / rotation / scale in the character window's coordinate
/// space. Updated each frame from `CharacterSettings::character_state`.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct CharacterTransform {
    #[expect(dead_code, reason = "Read by transform system in Phase 4")]
    pub translation: Vec3,
    #[expect(dead_code, reason = "Read by transform system in Phase 4")]
    pub rotation: Quat,
    #[expect(dead_code, reason = "Test reads `scale` in `bundle_defaults_are_safe`")]
    pub scale: f32,
}
