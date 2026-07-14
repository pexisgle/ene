//! VRM 1.0 character renderer for the ene desktop app.
//!
//! Platform-agnostic, built on `wgpu` 29 and the `gltf` 1.4 crate.
//! Full design in `docs/architecture/wgpu-migration.md`.
//!
//! ## Module map
//!
//! - [`prelude`] — curated "Supported API" re-exports (issue #6 of the [API refactor
//!   plan](https://github.com/pexisgle/Ene/blob/main/docs/architecture/api-refactor-plan.md)).
//!   Start here.
//! - [`camera`] — orthographic camera uniform + view-projection helpers.
//! - [`loader`] — read a `.vrm` from disk and produce a [`VrmModel`].
//! - [`model`] — public data types: [`VrmModel`], [`VrmMesh`], etc.
//! - [`expression`] — expression (blend-shape) layer.
//! - [`humanoid`] — humanoid bone registry (spec's 55 bones → glTF nodes).
//! - [`look_at`] — look-at properties + per-frame evaluator.
//! - [`expression_override`] — expression override semantics.
//! - [`renderer`] — wgpu render pipeline + bind group layouts.
//! - [`error`] — top-level error type.
//!
//! ## Supported API vs. internal
//!
//! Everything under `pub` remains callable (this crate makes no breaking-removal changes as
//! part of curating the surface), but not every `pub` item is meant for `ene-desktop` (or other
//! hosts) to use directly. [`prelude`] and `docs/api/ene-vrm.md` name the intentionally
//! supported subset — model loading, rendering, animation playback, look-at, and expressions.
//! Sub-parsers called only from [`loader::load_vrm`] (e.g. `load_humanoid_bones`,
//! `load_look_at`, `load_spring_bones`, `load_node_constraints`, `load_expression_overrides`,
//! `load_mtoon_materials`) and a handful of internal-only helpers/constants are marked
//! `#[doc(hidden)]` to keep the rendered API docs focused, even though their types remain `pub`
//! (some are unavoidably part of other supported types' public fields).

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod animation;
pub mod camera;
pub mod debug_renderer;
pub mod error;
pub mod expression;
pub mod expression_compositor;
pub mod expression_override;
pub mod humanoid;
pub mod layer_composer;
pub mod loader;
pub mod look_at;

pub mod model;
pub mod mtoon;
pub mod node_constraint;
pub mod post_process;
/// Curated "Supported API" re-exports — see the crate-level module map above.
pub mod prelude;
pub mod renderer;
pub mod spring_bone;

pub use animation::{
    BoneChannel, ExpressionChannel, Interpolation, LookAtChannel, RepeatMode, Sampler, VrmaAsset,
    VrmaClip, VrmaFrame, VrmaPlayer, VrmaProperties, evaluate_clip, load_vrma, quat_to_yaw_pitch,
    retarget_hips_translation, retarget_rotation,
};
pub use camera::{
    ModelUniform, OrthographicCamera, ndc_to_view_pos, ndc_to_view_pos_with_aspect, pixel_to_ndc,
    view_pos_to_world,
};
pub use debug_renderer::{
    CROSS_HALF_EXTENT, DEFAULT_LINE_CAPACITY, DebugLine, DebugRenderer, DebugVertex,
    SPHERE_LATITUDES, SPHERE_LONGITUDES, cross_lines, sphere_wireframe_lines_into,
};
pub use error::{VrmError, VrmResult};
pub use expression::{
    ExpressionLayer, ExpressionName, MAX_MORPH_TARGETS_PER_PRIMITIVE, PrimitiveMorphMeta,
    PrimitiveMorphs,
};
pub use expression_override::{
    BLINK_TARGET_NAMES, ExpressionDefinition, ExpressionOverrideSettings, ExpressionOverrideType,
    GAZE_TARGET_NAMES, MOUTH_TARGET_NAMES, apply_overrides, is_procedural,
    load_expression_overrides,
};
pub use humanoid::{
    BoneRestTransform, HUMANOID_BONE_NAMES, HumanoidBoneEntry, HumanoidBoneRegistry, VrmBone,
    canonicalize_bone_name, load_humanoid_bones,
};
pub use loader::load_vrm;
pub use look_at::{
    LookAtBoneDelta, LookAtBoneOutput, LookAtDirection, LookAtEvaluator, LookAtExpressionOutput,
    LookAtOutput, LookAtProperties, LookAtRangeMap, LookAtRangeMapSet, LookAtType, load_look_at,
};

pub use model::{AlphaMode, MeshVertex, NodeHierarchy, Skeleton, VrmMesh, VrmModel, VrmTexture};
pub use mtoon::{
    MToonGpuTextures, MToonMaterial, MToonTextureRef, MToonUniform, OutlineWidthMode, flags,
    load_mtoon_materials, texture_flags,
};
pub use node_constraint::{
    AimAxis, ConstraintEntry, NodeConstraint, NodeConstraintRegistry, RollAxis,
    load_node_constraints,
};
pub use renderer::VrmRenderer;
pub use spring_bone::{
    DEFAULT_DRAG_FORCE, DEFAULT_GRAVITY_DIR, DEFAULT_GRAVITY_POWER, DEFAULT_HIT_RADIUS,
    DEFAULT_STIFFNESS, SpringBoneChain, SpringBoneChainState, SpringBoneCollider,
    SpringBoneColliderGroup, SpringBoneJoint, SpringBoneJointState, SpringBoneProperties,
    SpringBoneShape, SpringBoneSimulator, load_spring_bones,
};

/// Returns the crate version. Useful for diagnostics and the `about` panel.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
