//! `ene-vrm` — VRM 1.0 character renderer for the ene desktop app.
//!
//! This crate is the platform-agnostic replacement for `bevy_vrm1`. It is
//! built directly on top of `wgpu` 29 and the `gltf` 1.4 crate. The full
//! design is documented in `docs/architecture/wgpu-migration.md`.
//!
//! ## PR3 scope
//!
//! PR3 ships the **minimum viable** VRM pipeline:
//!
//! - Load a `.vrm` (glTF binary with the `VRMC_vrm` extension) and
//!   extract the first mesh, the first base-color texture, and the
//!   skeleton (with **identity** skinning; the actual joint math ships
//!   with PR4+).
//! - Render the mesh with a basic PBR-lite WGSL shader (diffuse + lit
//!   + alpha) through an orthographic camera. The full MToon shader
//!     (rim / matcap / outline / emission) ships as a follow-up PR.
//! - No animation, no expressions, no look-at, no drag, no spring
//!   bone, no multi-mesh / multi-material handling. All of those are
//!   PR4 or later.
//!
//! ## Module map
//!
//! - [`camera`] — orthographic camera uniform + view-projection
//!   helpers. Lives at the crate root because the renderer is the
//!   only consumer and pulling it out keeps the import surface small.
//! - [`loader`] — read a `.vrm` file from disk and produce a
//!   [`VrmModel`]. Buffers are uploaded to the GPU; the returned
//!   `VrmModel` owns them.
//! - [`model`] — public data types: [`VrmModel`], [`VrmMesh`],
//!   [`VrmTexture`], [`Skeleton`].
//! - [`expression`] — PR4.4 expression (blend-shape) layer.
//!   Per-primitive morph targets and the global
//!   `BTreeMap<ExpressionName, f32>` of weights.
//! - [`humanoid`] — PR4.7 humanoid bone registry. Maps the
//!   spec's 55 bone names to glTF `Node` indices and
//!   per-bone rest transforms.
//! - [`look_at`] — PR4.8 look-at properties (range map +
//!   type discriminator) and the per-frame
//!   `(head_world, target_world) → (yaw, pitch)` evaluator.
//!   Produces per-bone deltas for `"bone"`-type models and
//!   morph weights for `"expression"`-type models.
//! - [`expression_override`] — PR4.9 expression override
//!   semantics (`isBinary`, `overrideMouth/Blink/LookAt`).
//!   The per-frame override evaluator applies the spec's
//!   `block` / `blend` rules to procedural targets.
//! - [`renderer`] — wgpu render pipeline + bind group layouts that
//!   can render a [`VrmModel`] into a `wgpu::TextureView`.
//! - [`error`] — top-level error type.

#![warn(missing_docs)]

pub mod camera;
pub mod error;
pub mod expression;
pub mod expression_override;
pub mod humanoid;
pub mod loader;
pub mod look_at;
pub mod model;
pub mod renderer;

pub use camera::{ModelUniform, OrthographicCamera};
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
pub use model::{Skeleton, VrmMesh, VrmModel, VrmTexture};
pub use renderer::VrmRenderer;

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
