//! GPU-side data types produced by [`crate::loader::load_vrm`].
//!
//! The loader loads **every primitive of every mesh** in the glTF
//! document. A VRM 1.0 model like AliciaSolid.vrm has 12 separate
//! glTF Mesh objects (body, clothes, hair, face, accessories,
//! etc.) — not one mesh with 12 primitives — so the loader walks
//! every `Mesh` and every `Primitive` rather than iterating only
//! `meshes[0]`, which would render the head/face area only.
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};

use crate::animation::VrmaFrame;
use crate::expression::ExpressionLayer;
use crate::expression_override::ExpressionDefinition;
use crate::humanoid::HumanoidBoneRegistry;
use crate::look_at::{LookAtBoneOutput, LookAtProperties};
use crate::mtoon::{MToonGpuTextures, MToonMaterial};
use crate::node_constraint::NodeConstraintRegistry;
use crate::spring_bone::SpringBoneProperties;

/// Which VRM dialect a loaded [`VrmModel`] came from.
///
/// Detected from the glTF root extension at load time:
/// `VRMC_vrm` means VRM 1.0; `VRM` means legacy VRM 0.x
/// (converted into the same runtime structures before loading).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VrmFormatVersion {
    /// Legacy VRM 0.x document (root extension `VRM`), converted to the
    /// unified runtime representation at load time.
    V0x,
    /// VRM 1.0 document (root extension `VRMC_vrm`); parsed by the native
    /// 1.0 loaders unchanged.
    #[default]
    V1,
}

impl VrmFormatVersion {
    /// Human-readable label used by GUI surfaces that show which dialect a
    /// loaded avatar uses.
    #[must_use]
    pub const fn as_label(self) -> &'static str {
        match self {
            Self::V0x => "VRM 0.x",
            Self::V1 => "VRM 1.0",
        }
    }
}

#[derive(Debug)]
pub struct VrmPrimitive {
    pub vertex_buf: wgpu::Buffer,
    pub vertex_count: u32,
    pub index_buf: wgpu::Buffer,
    pub index_count: u32,
    /// CPU-side mirror of the vertex data, kept after the GPU
    /// upload so CPU consumers ( collider builder, future
    /// baking passes) can read positions, joints, and weights
    /// without a GPU readback. Typical VRM 1.0 models weigh
    /// in at ~30k vertices × 64 bytes = ~2 MB; the convenience
    /// is worth the resident cost.
    ///
    /// `vertices[i].position` is in raw glTF space (the same
    /// frame the `vertex_buf` uses); consumers that want the
    /// normalised frame must apply `T(-center) * S(normalize_scale)`
    /// themselves, exactly like the per-frame `model.model` matrix
    /// does. The collider builder in `ene-desktop-v2` does this
    /// fold before computing per-bone sizes.
    pub vertices: Vec<MeshVertex>,
    /// Base-color texture for this primitive, if its material has
    /// one. `None` falls back to a flat color in the shader.
    pub base_color: Option<std::sync::Arc<VrmTexture>>,
    /// Alpha blending mode parsed from the glTF material's
    /// `alphaMode`. Controls draw order and pipeline selection
    /// in the renderer.
    pub alpha_mode: AlphaMode,
    /// Whether this primitive's material declares
    /// `KHR_materials_unlit`. Unlit primitives skip all lighting
    /// in the fragment shader and output the base color directly.
    pub unlit: bool,
    /// Parsed `VRMC_materials_mtoon` parameters. `None`
    /// for materials without the extension (the renderer falls
    /// back to the half-Lambert lite shader in that case).
    pub mtoon: Option<MToonMaterial>,
    /// GPU textures for `MToon` (shade multiply, shading
    /// shift, emissive, matcap, rim multiply, outline width,
    /// UV animation mask). `None` when the material has no `MToon`
    /// extension or no `MToon` textures.
    pub mtoon_textures: Option<std::sync::Arc<MToonGpuTextures>>,
}

/// Alpha blending mode of a primitive's material, parsed from
/// glTF `material.alphaMode`. Controls draw order and pipeline
/// selection in the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum AlphaMode {
    /// Opaque surface — depth write enabled, no blending.
    #[default]
    Opaque,
    /// Alpha cutout — depth write enabled, alpha test in shader.
    /// Currently rendered with the same pipeline as
    /// [`AlphaMode::Opaque`] (the alpha-test shader variant is a
    /// follow-up).
    Mask,
    /// Transparent blend — depth write disabled,
    /// pre-multiplied alpha blending. Drawn after opaque/mask
    /// primitives so the depth buffer is already filled.
    Blend,
}

impl AlphaMode {
    #[inline]
    pub const fn render_phase(self) -> u8 {
        match self {
            Self::Opaque | Self::Mask => 0,
            Self::Blend => 1,
        }
    }
}

/// A single glTF mesh object, as a list of primitives. The
/// loader walks every `Mesh` in the glTF document — a VRM 1.0 has
/// ~12 of these (body, `hair_front`, `hair_back`, face,
/// `clothes_top`, `clothes_bottom`, etc.), one per body part.
/// Loading only `meshes[0]` would render the head/face area only.
#[derive(Debug, Default)]
pub struct VrmMesh {
    pub primitives: Vec<VrmPrimitive>,
}

#[derive(Debug)]
pub struct VrmTexture {
    pub texture: wgpu::Texture,
    /// Default sampler (linear filtering, clamp-to-edge).
    pub sampler: wgpu::Sampler,
    /// Bind group layout `(1)` — used by the renderer to build the
    /// per-primitive bind group.
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

/// Skeleton metadata loaded from the first skin in the glTF. A
/// model without a skin is rendered with **identity** skinning;
/// this struct exposes the full joint list plus the per-joint
/// `inverse_bind` so the runtime can build the `mat4x4[]` skin
/// palette via the standard glTF formula
/// `joint_world * inverse_bind`. The pre-baked `bind_matrices`
/// field stores `inverse_bind[i]⁻¹`; the per-frame runtime matrix
/// is `current_joint_world * inverse_bind[i]` — **not**
/// `* bind_matrices[i]`, which would invert the bind transform
/// and break the rest pose.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    /// Inverse-bind matrices, one per joint. Loaded from
    /// `skin.inverse_bind_matrices` in the glTF. Combined
    /// with the joint's current world transform as
    /// `joint_world * inverse_bind[i]` to produce the
    /// per-joint skin matrix the shader reads.
    pub inverse_bind: Vec<Mat4>,
    /// `inverse_bind[i].inverse()` — the per-joint bind
    /// matrix (i.e. the joint's world transform at rest).
    /// The renderer uploads these as the static rest-pose
    /// palette initial value, but the per-frame palette is
    /// built from `inverse_bind` directly — see
    /// [`VrmModel::update_skin_palette`].
    pub bind_matrices: Vec<Mat4>,
    /// For every joint, the index of the glTF `Node` that
    /// owns it (loaded from `skin.joints()`). The runtime
    /// uses this to look up the joint's current world
    /// transform in [`NodeHierarchy::world_rotations`] /
    /// [`NodeHierarchy::world_positions`].
    pub joint_to_node: Vec<usize>,
}

impl Skeleton {
    pub const fn joint_count(&self) -> usize {
        self.inverse_bind.len()
    }
}

/// Full glTF node hierarchy, captured at load time so the
/// runtime can recompute per-joint world transforms when a
/// VRMA bone rotation lands. Index = glTF `Node` index.
///
/// The data is computed once from the glTF `Node` transforms
/// (local rotations / positions + parent indices) and the
/// resulting world-space values. This struct is
/// the bridge between `VrmaFrame::bone_rotations` (keyed by
/// canonical bone name) and the skin palette the GPU reads
/// every frame.
///
/// Four groups of fields exist because both look-ups happen:
///
/// 1. **Apply a frame**: `local_rotations[i]` and
///    `local_positions[i]` are the per-node buffer the
///    runtime overwrites every frame (from
///    `rest_local_rotations` + the frame's bone
///    rotations). The override is then propagated up via
///    `parents[i]` (or `-1` for root) and
///    `compute_world_transforms` to fill `world_rotations`
///    and `world_positions`.
///
/// 2. **Reset to rest pose**: `rest_local_rotations` and
///    `rest_local_positions` are the glTF node transforms
///    captured at load time. `update_skin_palette` copies
///    them back into `local_rotations` / `local_positions`
///    at the start of every frame so bones that the VRMA
///    doesn't animate stay in their T-pose (otherwise the
///    "missing" bones would carry over the previous frame's
///    rotation and the rest pose would slowly drift).
///
/// 3. **Look up a joint's world matrix**: given a glTF node
///    index `n`, read `Mat4::from_rotation_translation(
///    world_rotations[n], world_positions[n])`.
///
/// 4. **Write into the skin palette**: for each skeleton
///    joint `j`, the palette entry is
///    `joint_world * inverse_bind[j]`, where `joint_world`
///    uses the glTF node index from
///    `skeleton.joint_to_node[j]`. This is the standard
///    glTF skinning identity — at rest the two factors
///    cancel and the palette becomes the per-joint identity,
///    so the bind mesh renders unchanged.
#[derive(Clone, Debug, Default)]
pub struct NodeHierarchy {
    /// Per-node local rotation **buffer** the runtime
    /// overwrites every frame. Index = glTF node.
    pub local_rotations: Vec<Quat>,
    /// Per-node local translation **buffer** the runtime
    /// overwrites every frame. Index = glTF node.
    pub local_positions: Vec<Vec3>,
    /// Per-node local rotation at rest, captured from the
    /// glTF `Node` transforms at load time. `update_skin_palette`
    /// copies this back into `local_rotations` at the start
    /// of every frame so the buffer can be mutated freely.
    pub rest_local_rotations: Vec<Quat>,
    /// Per-node local translation at rest, captured from the
    /// glTF `Node` transforms at load time.
    pub rest_local_positions: Vec<Vec3>,
    /// Per-node parent index, or `-1` for root nodes.
    pub parents: Vec<i32>,
    /// Per-node world rotation. Updated in place when a
    /// frame is applied; the runtime reads this to build
    /// the skin palette.
    pub world_rotations: Vec<Quat>,
    /// Per-node world translation. Same lifecycle as
    /// `world_rotations`.
    pub world_positions: Vec<Vec3>,
    /// World rotation at rest, captured at load. Used for NLR retargeting.
    pub rest_world_rotations: Vec<Quat>,
    /// World translation at rest, captured at load. Used for hips height scaling.
    pub rest_world_positions: Vec<Vec3>,
}

impl NodeHierarchy {
    pub const fn len(&self) -> usize {
        self.local_rotations.len()
    }

    /// `true` when no nodes were captured (the model file
    /// had zero glTF `Node` objects — malformed).
    pub const fn is_empty(&self) -> bool {
        self.local_rotations.is_empty()
    }

    /// Build the world transforms from `local_rotations` /
    /// `local_positions` / `parents`. Walks each node's parent
    /// chain root → node so out-of-order glTF node indices stay
    /// correct (same strategy as VRMA rest-pose computation).
    pub fn compute_world_transforms(&mut self) {
        let n = self.local_rotations.len();
        if n == 0 {
            return;
        }
        for i in 0..n {
            let mut chain = Vec::new();
            let mut cur = i as i32;
            loop {
                chain.push(cur as usize);
                let p = self.parents[cur as usize];
                if p < 0 {
                    break;
                }
                cur = p;
            }
            chain.reverse();

            let mut wr = Quat::IDENTITY;
            let mut wp = Vec3::ZERO;
            for &node in &chain {
                wp += wr * self.local_positions[node];
                wr *= self.local_rotations[node];
            }
            self.world_rotations[i] = wr;
            self.world_positions[i] = wp;
        }
    }
}

/// Top-level loaded model. Owns all GPU resources needed to render
/// the VRM once.
#[derive(Debug)]
pub struct VrmModel {
    /// All glTF meshes in the file. The loader walks every `Mesh` —
    /// VRM 1.0 uses one glTF Mesh per body part rather than one
    /// Mesh with many primitives.
    pub meshes: Vec<VrmMesh>,
    /// Skeleton metadata. Not consumed by the renderer.
    pub skeleton: Skeleton,
    /// Raw glTF AABB `(min, max)` of every vertex, in model-local
    /// space (i.e. the same space the vertex buffer and the bind
    /// matrices use). The runtime uses this together with
    /// [`Self::center`] / [`Self::normalize_scale`] to build the
    /// `model.model` matrix and to fit the model into the
    /// viewport.
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    /// AABB center in raw glTF space. The runtime folds this
    /// into the model matrix as `T(-center) * S(normalize_scale) *
    /// T(position)` so the mesh ends up centered on the
    /// character's world position with a unit-ish extent.
    center: [f32; 3],
    /// `1.5 / max_extent` — the uniform scale that maps the
    /// longest AABB axis to the canonical 1.5 m model size.
    /// Folding this into the model matrix is what keeps the
    /// bind matrices valid: we never re-center / re-scale the
    /// vertex buffer, so `inverse_bind` (which was computed
    /// against the raw positions) lines up with the GPU data.
    normalize_scale: f32,
    /// Expression / blend-shape layer. `Default` for
    /// models without morph targets; the renderer treats the
    /// empty layer as a no-op.
    pub expressions: ExpressionLayer,
    /// Humanoid bone registry. Built from the
    /// `VRMC_vrm.humanoid.humanBones` block — empty for
    /// models without humanoid metadata (e.g. legacy
    /// VRM 0.x). Consumers (`LookAt`, `SpringBone`,
    /// VRMA, `NodeConstraint`) use this to map bone
    /// names to glTF node / Skeleton joint indices.
    pub humanoid: HumanoidBoneRegistry,
    /// Full glTF node hierarchy. Captured at
    /// load time so the runtime can recompute per-joint
    /// world transforms when a VRMA bone rotation lands.
    /// Empty for models without glTF nodes (malformed).
    pub nodes: NodeHierarchy,
    /// Look-at properties. Parsed from the
    /// `VRMC_vrm.lookAt` block — `None` for models
    /// without the block (e.g. legacy VRM 0.x). The
    /// runtime falls back to [`LookAtProperties::default`]
    /// when this is `None` so a model missing the block
    /// still gets the spec-default 90→10 range map and
    /// `"bone"` consumer type.
    pub look_at: Option<LookAtProperties>,
    /// Per-expression override definitions.
    /// Parsed from the `VRMC_vrm.expressions.{preset,custom}.<name>`
    /// tree — `isBinary`, `overrideMouth`, `overrideBlink`,
    /// `overrideLookAt`. Empty for models without the
    /// `VRMC_vrm.expressions` block, in which case the
    /// override pass is a no-op.
    pub expressions_meta: Vec<ExpressionDefinition>,
    /// Node constraints. Parsed from
    /// `VRMC_node_constraint` on glTF nodes. Empty for
    /// models without the extension.
    pub node_constraints: NodeConstraintRegistry,
    /// Spring bone properties. Parsed from
    /// `VRMC_springBone`. `None` for models without the
    /// extension. The runtime creates a
    /// [`SpringBoneSimulator`](crate::spring_bone::SpringBoneSimulator)
    /// from this to drive hair / cloth sway.
    pub spring_bones: Option<SpringBoneProperties>,
    /// Reusable skin palette buffer. Stored on the model to
    /// avoid a per-frame `Vec::with_capacity` allocation in
    /// [`Self::rebuild_skin_palette`].
    skin_palette: Vec<Mat4>,
    /// VRM dialect detected at load time. Defaults to
    /// [`VrmFormatVersion::V1`] so synthetic models built with
    /// [`Self::new`] keep the historical behaviour unless they opt in
    /// via [`Self::with_format_version`].
    format_version: VrmFormatVersion,
}

impl VrmModel {
    /// Raw glTF AABB `(min, max)` of every vertex, in model-local
    /// space. The runtime's auto-fit scale and the
    /// `model.model` matrix both consume this.
    pub const fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        (self.aabb_min, self.aabb_max)
    }

    /// AABB center in raw glTF space. The runtime subtracts
    /// this from every vertex (via the model matrix) so the
    /// character pivots around its own midpoint.
    pub const fn center(&self) -> [f32; 3] {
        self.center
    }

    /// `1.5 / max_extent` — the uniform scale the runtime
    /// applies to map the longest AABB axis to the canonical
    /// 1.5 m model size.
    pub const fn normalize_scale(&self) -> f32 {
        self.normalize_scale
    }

    /// AABB expressed in **normalized** space (i.e. after
    /// applying `T(-center) * S(normalize_scale)` to the raw
    /// AABB). The result is centred on the origin with the
    /// longest extent equal to 1.5. The auto-fit helper
    /// consumes this; tests use it to avoid having to
    /// hard-code the raw AABB.
    pub fn normalized_aabb(&self) -> ([f32; 3], [f32; 3]) {
        let s = self.normalize_scale;
        let c = self.center;
        let n =
            |v: [f32; 3]| -> [f32; 3] { [(v[0] - c[0]) * s, (v[1] - c[1]) * s, (v[2] - c[2]) * s] };
        (n(self.aabb_min), n(self.aabb_max))
    }

    pub const fn joint_count(&self) -> usize {
        self.skeleton.joint_count()
    }

    /// The runtime writes into
    /// `expressions.weights` every frame; the renderer reads it.
    pub const fn expressions(&self) -> &ExpressionLayer {
        &self.expressions
    }

    /// Used by
    /// `CharacterRenderer::apply_emotions` in
    /// `apps/ene-desktop-v2` to push the latest emotion weights
    /// into the model.
    pub const fn expressions_mut(&mut self) -> &mut ExpressionLayer {
        &mut self.expressions
    }

    /// `None` for models
    /// without the `VRMC_vrm.lookAt` block (e.g. legacy
    /// VRM 0.x). The desktop runtime supplies the spec
    /// default in that case via
    /// [`LookAtProperties::default`].
    pub const fn look_at(&self) -> Option<&LookAtProperties> {
        self.look_at.as_ref()
    }

    /// The VRM dialect this model was parsed from.
    #[must_use]
    pub const fn format_version(&self) -> VrmFormatVersion {
        self.format_version
    }

    /// Human-readable dialect label ([`VrmFormatVersion::as_label`]).
    #[must_use]
    pub const fn format_version_label(&self) -> &'static str {
        self.format_version.as_label()
    }

    /// Override the detected dialect. Test fixtures that build a
    /// [`VrmModel`] directly keep the default; the loader chains this after
    /// [`Self::new`] to record what it found.
    #[must_use]
    pub const fn with_format_version(mut self, format_version: VrmFormatVersion) -> Self {
        self.format_version = format_version;
        self
    }

    /// Construct a `VrmModel` from its already-built pieces plus
    /// the raw AABB and the centre/scale the runtime needs to
    /// build the model matrix. Used by the loader and by
    /// downstream test fixtures (the `ene-desktop-v2` camera
    /// target test attaches a minimal synthetic model so it can
    /// mutate `nodes.world_positions` and assert the camera
    /// target is unaffected by animation).
    #[must_use]
    pub const fn new(
        meshes: Vec<VrmMesh>,
        skeleton: Skeleton,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
        center: [f32; 3],
        normalize_scale: f32,
        expressions: ExpressionLayer,
        humanoid: HumanoidBoneRegistry,
        nodes: NodeHierarchy,
        look_at: Option<LookAtProperties>,
        expressions_meta: Vec<ExpressionDefinition>,
        node_constraints: NodeConstraintRegistry,
        spring_bones: Option<SpringBoneProperties>,
    ) -> Self {
        Self {
            meshes,
            skeleton,
            aabb_min,
            aabb_max,
            center,
            normalize_scale,
            expressions,
            humanoid,
            nodes,
            look_at,
            expressions_meta,
            node_constraints,
            spring_bones,
            skin_palette: Vec::new(),
            format_version: VrmFormatVersion::V1,
        }
    }

    /// Apply a [`VrmaFrame`] to the node hierarchy and return
    /// the per-skin-joint palette `mat4x4<f32>[]` ready for
    /// `queue.write_buffer`.
    ///
    /// The algorithm is:
    ///
    /// 1. **Reset to rest**: copy `nodes.rest_local_rotations` /
    ///    `nodes.rest_local_positions` back into the
    ///    `local_rotations` / `local_positions` buffers (the
    ///    per-frame overrides live there). The buffer-to-buffer
    ///    copy on its own is **not** a true rest reset — the
    ///    previous frame's bone rotations persist in
    ///    `local_*` — so we have to copy from
    ///    `rest_local_*` instead.
    /// 2. **Apply frame rotations**: for every
    ///    `(bone_name, retargeted_rotation)` in the frame,
    ///    look the bone up in `self.humanoid`. If found and
    ///    the entry carries a `joint` index, override the
    ///    local rotation of `nodes.local_rotations[entry.node]`
    ///    with the VRMA's bone rotation. Bones that aren't
    ///    in the humanoid registry are silently dropped.
    /// 3. **Apply `LookAt` bone deltas**: for the
    ///    `head` / `leftEye` / `rightEye` humanoid bones
    ///    whose [`LookAtBoneOutput`] carries a non-identity
    ///    delta, multiply the delta onto the **current** local
    ///    rotation (`current * look_at_delta`). The `LookAt` step
    ///    runs **after** the VRMA step so cursor tracking is
    ///    additive on top of the motion pose — overwriting with
    ///    `rest * delta` would discard head compensation that
    ///    body-bowing clips rely on (e.g. `VRMA_02`), leaving the
    ///    character staring at the floor whenever `LookAt` is
    ///    active. Bones missing from the humanoid registry are
    ///    silently dropped, so the call is a no-op on models
    ///    without humanoid metadata.
    /// 4. **Walk the hierarchy**: `nodes.compute_world_transforms()`
    ///    fills `world_rotations` / `world_positions`.
    /// 5. **Hips translation**: if the frame carries a dest **local**
    ///    `hips_translation`, write it to `local_positions[hips]`
    ///    *before* the world walk. VRMA translation channels are
    ///    absolute local glTF values, not world deltas.
    /// 6. **Build the palette**: for every skeleton joint
    ///    `j`, palette\[j\] = `joint_world * inverse_bind[j]`,
    ///    where `joint_world` uses the glTF node index from
    ///    `skeleton.joint_to_node[j]`. A node index of
    ///    `usize::MAX` (the loader's sentinel for
    ///    "joint outside the glTF node list") is treated
    ///    as identity. This is the standard glTF skinning
    ///    formula — at rest the two factors cancel and the
    ///    per-joint palette becomes identity, so the bind
    ///    mesh renders unchanged.
    ///
    /// **Retargeting**: callers should pass dest-local rotations and
    /// hips from [`crate::animation::evaluate_retargeted`]. Raw
    /// [`crate::animation::evaluate_clip`] samples are source-local
    /// and will double hips height if added onto rest world Y.
    ///
    /// **`LookAt` semantics**: `look_at` is the
    /// [`LookAtBoneOutput`] of the current frame, or `None`
    /// when the model is `"expression"`-type (the `LookAt`
    /// signal then routes into morph weights via
    /// [`crate::expression::ExpressionLayer`], not into bone
    /// rotations) or when no cursor sample has been
    /// processed yet. `None` makes step 3 a no-op.
    ///
    /// Returns an empty slice when the model has zero
    /// skeleton joints — the renderer's identity palette
    /// stays in effect and no GPU write is needed.
    pub fn update_skin_palette(
        &mut self,
        frame: &VrmaFrame,
        look_at: Option<&LookAtBoneOutput>,
    ) -> &[Mat4] {
        let joint_count = self.skeleton.joint_count();
        if joint_count == 0 || self.nodes.is_empty() {
            self.skin_palette.clear();
            return &self.skin_palette;
        }

        // 1. Reset to rest. We have to copy from
        //    `rest_local_*` (the loader-captured pose), not
        //    from `local_*` itself — `local_*` was mutated
        //    by the previous frame's call and would
        //    otherwise carry the previous frame's bone
        //    rotations into the new frame.
        self.nodes
            .local_rotations
            .copy_from_slice(&self.nodes.rest_local_rotations);
        self.nodes
            .local_positions
            .copy_from_slice(&self.nodes.rest_local_positions);
        self.nodes
            .world_rotations
            .copy_from_slice(&self.nodes.local_rotations);
        self.nodes
            .world_positions
            .copy_from_slice(&self.nodes.local_positions);

        // 2. Apply frame rotations. `by_name` canonicalises
        //    the bone name (the `VrmaFrame::bone_rotations`
        //    keys are already canonical, but `by_name` is
        //    the public lookup path and matches what
        //    LookAt / SpringBone use elsewhere).
        for (bone_name, rot) in &frame.bone_rotations {
            let Some(entry) = self.humanoid.by_name(bone_name) else {
                continue;
            };
            self.nodes.local_rotations[entry.node] = *rot;
        }

        // 2.5 apply the LookAt cursor-tracking deltas
        //    on top of the VRMA pose. Multiply onto the current
        //    local rotation (already VRMA or rest) so body-bowing
        //    motions keep their head compensation. Identity
        //    deltas are skipped. Unknown humanoid bone names are
        //    silently dropped — `by_name` returns `None` and the
        //    existing local rotation wins.
        if let Some(look_at) = look_at {
            for (bone_name, delta) in [
                ("head", look_at.head),
                ("leftEye", look_at.left_eye),
                ("rightEye", look_at.right_eye),
            ] {
                if delta.delta == Quat::IDENTITY {
                    continue;
                }
                let Some(entry) = self.humanoid.by_name(bone_name) else {
                    continue;
                };
                if entry.node >= self.nodes.local_rotations.len() {
                    continue;
                }
                self.nodes.local_rotations[entry.node] *= delta.delta;
            }
        }

        self.rebuild_skin_palette(frame.hips_translation)
    }

    /// Recompute world transforms from the current `local_*`
    /// buffers and rebuild the skin palette.
    ///
    /// Call after mutating [`NodeHierarchy::local_rotations`]
    /// post-pose (e.g. spring bones) so the same-frame mesh
    /// reflects those rotations. `hips_translation` is dest
    /// **local** hips translation (not a world delta).
    pub fn rebuild_skin_palette(&mut self, hips_translation: Option<Vec3>) -> &[Mat4] {
        let joint_count = self.skeleton.joint_count();
        if joint_count == 0 || self.nodes.is_empty() {
            self.skin_palette.clear();
            return &self.skin_palette;
        }

        if let Some(hips_local) = hips_translation
            && let Some(hips_entry) = self.humanoid.hips()
            && hips_entry.node < self.nodes.local_positions.len()
        {
            self.nodes.local_positions[hips_entry.node] = hips_local;
        }

        self.nodes.compute_world_transforms();

        // Build the palette. The standard glTF skinning
        // identity is `joint_world * inverse_bind[j]`, which
        // is identity at rest. Reuse the stored buffer to avoid
        // a per-frame allocation.
        self.skin_palette.clear();
        self.skin_palette.reserve(joint_count);
        for j in 0..joint_count {
            let node_idx = self
                .skeleton
                .joint_to_node
                .get(j)
                .copied()
                .unwrap_or(usize::MAX);
            let joint_world = if node_idx == usize::MAX || node_idx >= self.nodes.len() {
                Mat4::IDENTITY
            } else {
                Mat4::from_rotation_translation(
                    self.nodes.world_rotations[node_idx],
                    self.nodes.world_positions[node_idx],
                )
            };
            let inverse_bind = self
                .skeleton
                .inverse_bind
                .get(j)
                .copied()
                .unwrap_or(Mat4::IDENTITY);
            self.skin_palette.push(joint_world * inverse_bind);
        }
        &self.skin_palette
    }

    /// Sample a VRMA clip with NLR retargeting onto this model's rest pose.
    #[must_use]
    pub fn evaluate_vrma(
        &self,
        asset: &crate::animation::VrmaAsset,
        clip: &crate::animation::VrmaClip,
        t: f32,
    ) -> VrmaFrame {
        crate::animation::evaluate_retargeted(
            asset,
            clip,
            t,
            &self.humanoid,
            crate::animation::DestRestPose {
                local_rotations: &self.nodes.rest_local_rotations,
                world_rotations: &self.nodes.rest_world_rotations,
                local_positions: &self.nodes.rest_local_positions,
                world_positions: &self.nodes.rest_world_positions,
            },
        )
    }
}

/// Maximum number of joints that can influence a single vertex.
/// glTF 2.0 / VRM 1.0 standardise on 4. The shader's `weights`
/// attribute is `vec4<f32>`; the `joints` attribute is `vec4<u32>`.
pub const MAX_JOINTS_PER_VERTEX: usize = 4;

/// Per-vertex layout used by the loader and the shader.
///
/// 60 bytes (pos vec3 + uv vec2 + normal vec3 + joints vec4 + weights vec4).
/// Models that don't define `JOINTS_0` / `WEIGHTS_0` fall back to
/// `joints = [0, 0, 0, 0]`, `weights = [1, 0, 0, 0]` and the
/// renderer uploads a one-element `skin[]` containing
/// `Mat4::IDENTITY`, so the per-vertex math reduces to
/// `weights[0] * skin[0] * pos = pos`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
    /// Per-vertex joint indices, up to `MAX_JOINTS_PER_VERTEX`.
    /// Stored as `u32` so 256+-joint humanoid models (e.g. models
    /// with per-finger bones) can address every joint without
    /// silent aliasing onto `skin_matrices[255]`. The vertex
    /// buffer grew by 12 bytes per vertex (from's `[u8; 4]`
    /// → `[u32; 4]`); the `LAYOUT` attribute 3 is
    /// `Float32x4 → Uint32x4` and the WGSL `vec4<u32>` is
    /// unchanged.
    pub joints: [u32; 4],
    /// Per-vertex joint weights, `sum(weights) = 1.0` per vertex.
    pub weights: [f32; 4],
}

impl MeshVertex {
    /// Vertex buffer layout entry shared by the loader and the
    /// renderer. Attribute locations match `shaders/mtoon_skinned.wgsl`:
    ///
    /// - `0` = position (`vec3<f32>`)
    /// - `1` = uv (`vec2<f32>`)
    /// - `2` = normal (`vec3<f32>`)
    /// - `3` = joints (`vec4<u32>`, uploaded as `Uint32x4`)
    /// - `4` = weights (`vec4<f32>`)
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x2,
            2 => Float32x3,
            3 => Uint32x4,
            4 => Float32x4,
        ],
    };

    pub fn as_bytes(vertices: &[Self]) -> &[u8] {
        bytemuck::cast_slice(vertices)
    }
}

// Compile-time guard: a `[u8; 4]` `joints` field would silently
// alias every joint >= 255 onto `skin_matrices[255]`.
const _: () = {
    const JOINTS_SIZE: usize = std::mem::size_of::<[u32; MAX_JOINTS_PER_VERTEX]>();
    assert!(
        JOINTS_SIZE == 16,
        "MeshVertex::joints must be a 16-byte `vec4<u32>` attribute; check MAX_JOINTS_PER_VERTEX and the field type"
    );
    const VERTEX_SIZE: usize = std::mem::size_of::<MeshVertex>();
    assert!(
        VERTEX_SIZE == 60 || VERTEX_SIZE == 64,
        "MeshVertex size drifted; the LAYOUT attribute stride will be wrong"
    );
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid::{BoneRestTransform, HumanoidBoneEntry, HumanoidBoneRegistry, VrmBone};
    use crate::look_at::{LookAtBoneDelta, LookAtBoneOutput};

    /// Build a `NodeHierarchy` with a single root node at
    /// `pos` rotated by `rot`. The world buffers start at
    /// zero so the test can assert that `compute_world_transforms`
    /// overwrites them from the local buffers.
    fn single_node_hierarchy(rot: Quat, pos: Vec3) -> NodeHierarchy {
        NodeHierarchy {
            local_rotations: vec![rot],
            local_positions: vec![pos],
            rest_local_rotations: vec![rot],
            rest_local_positions: vec![pos],
            parents: vec![-1],
            world_rotations: vec![Quat::IDENTITY],
            world_positions: vec![Vec3::ZERO],
            rest_world_rotations: vec![rot],
            rest_world_positions: vec![pos],
        }
    }

    /// Build a `NodeHierarchy` from a list of
    /// `(local_rot, local_pos, parent_index_or_negative_one)`.
    /// World buffers are pre-sized to `IDENTITY` / `ZERO` so
    /// the test can detect whether `compute_world_transforms`
    /// overwrote them. The rest buffers are seeded from the
    /// local buffers so the `update_skin_palette` reset works
    /// in tests that don't explicitly construct the rest pose.
    fn hierarchy_from_spec(nodes: &[(Quat, Vec3, i32)]) -> NodeHierarchy {
        let mut h = NodeHierarchy {
            local_rotations: Vec::with_capacity(nodes.len()),
            local_positions: Vec::with_capacity(nodes.len()),
            rest_local_rotations: Vec::with_capacity(nodes.len()),
            rest_local_positions: Vec::with_capacity(nodes.len()),
            parents: Vec::with_capacity(nodes.len()),
            world_rotations: Vec::with_capacity(nodes.len()),
            world_positions: Vec::with_capacity(nodes.len()),
            rest_world_rotations: Vec::with_capacity(nodes.len()),
            rest_world_positions: Vec::with_capacity(nodes.len()),
        };
        for (r, p, parent) in nodes {
            h.local_rotations.push(*r);
            h.local_positions.push(*p);
            h.rest_local_rotations.push(*r);
            h.rest_local_positions.push(*p);
            h.parents.push(*parent);
            h.world_rotations.push(Quat::IDENTITY);
            h.world_positions.push(Vec3::ZERO);
            h.rest_world_rotations.push(Quat::IDENTITY);
            h.rest_world_positions.push(Vec3::ZERO);
        }
        h.compute_world_transforms();
        h.rest_world_rotations.clone_from(&h.world_rotations);
        h.rest_world_positions.clone_from(&h.world_positions);
        h
    }

    /// Build a `VrmModel` with a 1-joint skeleton that points
    /// at the first node. The skeleton's `inverse_bind` is the
    /// identity matrix (so the per-joint rest `bind_matrices[j]
    /// = inverse_bind[j].inverse()` is also identity) which
    /// keeps the test arithmetic simple.
    fn single_joint_model(humanoid: HumanoidBoneRegistry, nodes: NodeHierarchy) -> VrmModel {
        let bind = Mat4::IDENTITY;
        VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![bind],
                bind_matrices: vec![bind],
                joint_to_node: vec![0],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            nodes,
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        )
    }

    /// the `joints` attribute must be `Uint32x4` and
    /// `joints` field must be `[u32; 4]`. A regression to `[u8; 4]`
    /// would (a) pass the build thanks to the `Pod` / `Zeroable`
    /// blanket impls and (b) silently break 256+ joint models.
    /// This test pins the field type so the failure mode is a
    /// compile error at the test site, not a silent skinning
    /// glitch in the wild.
    #[test]
    fn joints_field_is_u32() {
        let v = MeshVertex {
            position: [0.0; 3],
            uv: [0.0; 2],
            normal: [0.0; 3],
            joints: [256, 257, 258, 259],
            weights: [0.0; 4],
        };
        // A `[u8; 4]` field could not hold 256..=259. The type
        // system enforces the invariant but we also assert at
        // runtime so the test acts as a fence against an
        // accidental `[u32; 4] → [u16; 4]` shrink that would
        // re-break the upper joint range.
        assert_eq!(v.joints[0], 256);
        assert_eq!(v.joints[3], 259);
    }

    /// Sanity: the vertex layout is `60..=64` bytes (the exact
    /// size depends on whether `wgpu` pads `vec3`/`vec2`
    /// attributes — both 60 and 64 are valid host-side sizes
    /// for the current `LAYOUT`).
    #[test]
    fn vertex_size_is_aligned_for_4_way_attribs() {
        let size = std::mem::size_of::<MeshVertex>();
        assert!(
            size == 60 || size == 64,
            "unexpected vertex size {size}; check MeshVertex fields and LAYOUT"
        );
    }

    /// A single root node should have its world transform
    /// equal to its local transform after the walk. The
    /// pre-existing world buffer was zeroed out to detect
    /// the overwrite.
    #[test]
    fn node_hierarchy_world_transforms_root_only() {
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let pos = Vec3::new(1.0, 2.0, 3.0);
        let mut h = single_node_hierarchy(rot, pos);
        h.compute_world_transforms();
        assert_eq!(h.world_rotations[0], rot);
        assert_eq!(h.world_positions[0], pos);
    }

    /// An empty hierarchy must not panic and must leave the
    /// buffers (none) untouched. This is the "model file
    /// had zero glTF `Node` objects — malformed" path; the
    /// loader emits a warning, the renderer later detects
    /// `is_empty()` in `update_skin_palette` and bails.
    #[test]
    fn node_hierarchy_world_transforms_empty_is_noop() {
        let mut h = NodeHierarchy::default();
        h.compute_world_transforms();
        assert!(h.is_empty());
    }

    /// A two-node chain: root at `(1, 0, 0)` and a child
    /// whose local translation is `(0, 1, 0)`. The child's
    /// world translation must be the sum of the parent and
    /// its own local translation; the child's world rotation
    /// is the parent's world rotation (identity here) times
    /// its own (also identity).
    #[test]
    fn node_hierarchy_world_transforms_chain_translates() {
        let mut h = hierarchy_from_spec(&[
            (Quat::IDENTITY, Vec3::new(1.0, 0.0, 0.0), -1),
            (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0), 0),
        ]);
        h.compute_world_transforms();
        assert_eq!(h.world_positions[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(h.world_positions[1], Vec3::new(1.0, 1.0, 0.0));
    }

    /// A two-node chain with the child rotated 90° around
    /// the Y axis. The child's local translation `(1, 0, 0)`
    /// is the child's position *in the parent's frame*
    /// (the TRS matrix maps a point from this node's local
    /// space to the parent's space, so the local
    /// translation is the node's offset in the parent's
    /// frame, not the child's). The 90° rotation therefore
    /// does **not** rotate the child's translation — the
    /// translation stays `(1, 0, 0)` in the parent's
    /// frame. The child's world position is
    /// `parent_pos + R_parent * t_local = (1, 0, 0) +
    /// IDENTITY * (1, 0, 0) = (2, 0, 0)`. The rotation
    /// does affect the world rotation, which is
    /// `R_parent * R_child = IDENTITY * rot_y90 = rot_y90`.
    #[test]
    fn node_hierarchy_world_transforms_chain_rotates_translation() {
        let rot_y90 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let mut h = hierarchy_from_spec(&[
            (Quat::IDENTITY, Vec3::new(1.0, 0.0, 0.0), -1),
            (rot_y90, Vec3::new(1.0, 0.0, 0.0), 0),
        ]);
        h.compute_world_transforms();
        assert_eq!(h.world_positions[0], Vec3::new(1.0, 0.0, 0.0));
        assert!(
            (h.world_positions[1] - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5,
            "expected child world pos (2, 0, 0), got {:?}",
            h.world_positions[1]
        );
        let expected_rot = rot_y90;
        let actual = h.world_rotations[1];
        let dot = actual.dot(expected_rot).abs();
        assert!(
            (dot - 1.0).abs() < 1e-5,
            "expected child world rot ≈ {expected_rot:?}, got {actual:?}"
        );
    }

    /// A three-node chain tests that the cascade is
    /// transitive (not just two-deep). Grandchild's world
    /// position must include the parent and grandparent
    /// contributions.
    #[test]
    fn node_hierarchy_world_transforms_three_node_chain() {
        let mut h = hierarchy_from_spec(&[
            (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0), -1),
            (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0), 0),
            (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0), 1),
        ]);
        h.compute_world_transforms();
        assert_eq!(h.world_positions[0], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(h.world_positions[1], Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(h.world_positions[2], Vec3::new(0.0, 3.0, 0.0));
    }

    /// A model with zero skeleton joints must return an
    /// empty `Vec` (the renderer's static identity palette
    /// stays in effect and no GPU write is required).
    #[test]
    fn update_skin_palette_empty_joints_returns_empty() {
        let model = VrmModel::new(
            Vec::new(),
            Skeleton::default(),
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            HumanoidBoneRegistry::new(),
            single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame::default();
        let (_owned, palette) = model_palette(model, &frame);
        assert!(palette.is_empty());
    }

    /// A model with an empty `NodeHierarchy` but a
    /// non-empty skeleton must also return an empty
    /// `Vec` (the model is missing its nodes; we cannot
    /// compute world transforms). The renderer's static
    /// identity palette stays in effect.
    #[test]
    fn update_skin_palette_empty_nodes_returns_empty() {
        let bind = Mat4::IDENTITY;
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![bind],
                bind_matrices: vec![bind],
                joint_to_node: vec![0],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            HumanoidBoneRegistry::new(),
            NodeHierarchy::default(),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame::default();
        let (_owned, palette) = model_palette(model, &frame);
        assert!(palette.is_empty());
    }

    /// With no bone rotations and no hips translation, the
    /// per-joint palette must equal the rest-pose `joint_world`
    /// \* `bind_matrices[j]`. For a single-joint model with
    /// identity `bind_matrices` and a single node at the
    /// origin, that is `Mat4::IDENTITY` for every joint.
    #[test]
    fn update_skin_palette_no_frame_changes_returns_rest_palette() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let model = single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let frame = VrmaFrame::default();
        let (_owned, palette) = model_palette(model, &frame);
        assert_eq!(palette.len(), 1);
        assert_eq!(palette[0], Mat4::IDENTITY);
    }

    /// A 90° Y rotation on the hips joint, with the joint
    /// at the origin and identity bind matrix, must produce
    /// a palette entry whose translation column is
    /// `(0, 0, 0)` (origin stays at origin) and whose
    /// rotation block is the 90° Y rotation. The test
    /// applies the resulting palette to `(1, 0, 0)` and
    /// checks it lands at `(0, 0, -1)`.
    #[test]
    fn update_skin_palette_applies_bone_rotation() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let model = single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let mut frame = VrmaFrame::default();
        frame.bone_rotations.insert(
            "hips".into(),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let (_owned, palette) = model_palette(model, &frame);
        let transformed = palette[0].transform_point3(Vec3::new(1.0, 0.0, 0.0));
        assert!(
            (transformed - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "expected +X to rotate to -Z, got {transformed:?}"
        );
    }

    #[test]
    fn update_skin_palette_hips_local_does_not_double_rest_height() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let rest = Vec3::new(0.0, 0.9, 0.0);
        let sample = Vec3::new(0.0, 0.95, 0.0);
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![Mat4::IDENTITY],
                bind_matrices: vec![Mat4::IDENTITY],
                joint_to_node: vec![0],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            single_node_hierarchy(Quat::IDENTITY, rest),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame {
            hips_translation: Some(sample),
            ..Default::default()
        };
        let (owned, palette) = model_palette(model, &frame);
        let p0 = palette[0].transform_point3(Vec3::ZERO);
        assert!(
            (p0 - sample).length() < 1e-5,
            "expected world hips {sample:?}, got {p0:?}"
        );
        assert!(
            (owned.nodes.local_positions[0] - sample).length() < 1e-5,
            "hips must be applied as local, got {:?}",
            owned.nodes.local_positions[0]
        );
    }

    /// Hips translation must cascade to every descendant
    /// node (the hips subtree in the glTF parent chain) but
    /// must not propagate to ancestors. A three-node model
    /// with the hips at node 0, a child at node 1, and a
    /// sibling at node 2: a hips translation of `(0, 1, 0)`
    /// moves both the child and the sibling by `(0, 1, 0)`
    /// (they're both descendants of the hips). The
    /// grandparent (which would be `parent < 0`) is **not**
    /// affected.
    #[test]
    fn update_skin_palette_hips_translation_cascades_to_descendants() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![Mat4::IDENTITY, Mat4::IDENTITY],
                bind_matrices: vec![Mat4::IDENTITY, Mat4::IDENTITY],
                joint_to_node: vec![0, 1],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            hierarchy_from_spec(&[
                (Quat::IDENTITY, Vec3::new(0.0, 0.0, 0.0), -1),
                (Quat::IDENTITY, Vec3::new(0.0, 1.0, 0.0), 0),
                (Quat::IDENTITY, Vec3::new(1.0, 0.0, 0.0), 0),
            ]),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame {
            hips_translation: Some(Vec3::new(0.0, 1.0, 0.0)),
            ..Default::default()
        };
        let (owned, palette) = model_palette(model, &frame);
        assert_eq!(palette.len(), 2);
        let p0 = palette[0].transform_point3(Vec3::ZERO);
        assert!(
            (p0 - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5,
            "expected hips world pos (0, 1, 0), got {p0:?}"
        );
        // Joint 1 (child of hips): world position (0, 2, 0)
        // — the child's rest is (0, 1, 0) and the hips
        // added (0, 1, 0) on top.
        let p1 = palette[1].transform_point3(Vec3::ZERO);
        assert!(
            (p1 - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-5,
            "expected child world pos (0, 2, 0), got {p1:?}"
        );
        // The third node (sibling of the child, also a
        // direct child of the hips) IS a descendant of the
        // hips and therefore gets the same `(0, 1, 0)`
        // translation. Its rest position is `(1, 0, 0)`,
        // so the cascaded world position is `(1, 1, 0)`.
        assert!(
            (owned.nodes.world_positions[2] - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-5,
            "expected sibling world pos (1, 1, 0), got {:?}",
            owned.nodes.world_positions[2]
        );
    }

    /// A frame referencing a bone name that is not in the
    /// humanoid registry must be silently dropped (the
    /// runtime can't find a node to apply it to). The
    /// palette must still be produced from the rest pose
    /// — this is the path taken when a VRMA animates a bone
    /// the target model doesn't have.
    #[test]
    fn update_skin_palette_ignores_unknown_bone_names() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let model = single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let mut frame = VrmaFrame::default();
        frame.bone_rotations.insert(
            "leftlittlepinkytoe".into(),
            Quat::from_rotation_x(std::f32::consts::PI),
        );
        let (_owned, palette) = model_palette(model, &frame);
        assert_eq!(palette.len(), 1);
        assert_eq!(palette[0], Mat4::IDENTITY);
    }

    /// The `usize::MAX` sentinel in `joint_to_node[j]` (the
    /// loader's "joint outside the glTF node list" value)
    /// must degrade to a per-joint identity contribution so
    /// the model renders without a panic. The bind matrix
    /// for that joint is then the only surviving factor.
    #[test]
    fn update_skin_palette_uses_identity_for_out_of_range_joint_node() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let bind = Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0));
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![bind],
                bind_matrices: vec![bind],
                joint_to_node: vec![usize::MAX],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame::default();
        let (_owned, palette) = model_palette(model, &frame);
        assert_eq!(palette.len(), 1);
        // With the world matrix falling back to identity,
        // the per-joint palette is `IDENTITY * bind` = bind.
        assert_eq!(palette[0], bind);
    }

    /// Guards the ordering of `inverse_bind` / `bind_matrices`:
    /// if `inverse_bind` held the bind matrix instead of its
    /// inverse, `update_skin_palette` would compute
    /// `joint_world * bind_matrix ≈ bind_matrix²` at rest,
    /// doubling every joint's translation and stretching the model
    /// vertically. With `inverse_bind` holding the IBM
    /// (`bind_matrix.inverse()`), the standard glTF identity
    /// `joint_world * inverse_bind` collapses to the identity
    /// matrix at rest.
    #[test]
    fn update_skin_palette_rest_palette_is_identity_for_non_unit_ibm() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("head".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        // Joint rest position = (0, 1, 0). The IBM is its inverse
        // so the standard identity at rest should hold.
        let joint_pos = Vec3::new(0.0, 1.0, 0.0);
        let ibm = Mat4::from_translation(-joint_pos);
        let bind_matrix = Mat4::from_translation(joint_pos);
        let model = VrmModel::new(
            Vec::new(),
            Skeleton {
                inverse_bind: vec![ibm],
                bind_matrices: vec![bind_matrix],
                joint_to_node: vec![0],
            },
            [-1.0, -1.0, -1.0],
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            1.0,
            ExpressionLayer::default(),
            humanoid,
            single_node_hierarchy(Quat::IDENTITY, joint_pos),
            None,
            Vec::new(),
            NodeConstraintRegistry::default(),
            None,
        );
        let frame = VrmaFrame::default();
        let (_owned, palette) = model_palette(model, &frame);
        assert_eq!(palette.len(), 1);
        // At rest, `joint_world * inverse_bind` must be identity
        // — NOT `bind_matrix * bind_matrix` (which is what the
        // bug produced and which would translate a vertex by
        // `2 * joint_pos`).
        assert_eq!(
            palette[0],
            Mat4::IDENTITY,
            "rest-pose palette must be identity, got {palette:?}"
        );
    }

    /// Helper: run `update_skin_palette` against a fresh
    /// owned copy of the model and return the palette plus
    /// the (mutated) model. Tests that need to assert
    /// against the world-transform side effects use the
    /// returned model. The `LookAt` argument defaults to
    /// `None`; tests that exercise the `LookAt` composition
    /// call `model.update_skin_palette(&frame, look_at)`
    /// directly.
    fn model_palette(mut model: VrmModel, frame: &VrmaFrame) -> (VrmModel, Vec<Mat4>) {
        let palette = model.update_skin_palette(frame, None).to_vec();
        (model, palette)
    }

    /// A `LookAt` delta on the `head` bone must rotate the
    /// head's local transform to `rest * delta` and the
    /// resulting world rotation must reach the palette.
    /// We construct a single-node model with the head as
    /// the only humanoid bone, drive a 90° Y `LookAt` delta,
    /// and assert that the per-joint palette rotates a
    /// +X point to -Z (the same assertion used in
    /// `update_skin_palette_applies_bone_rotation`, which
    /// proves the `LookAt` path ends up in the GPU-readable
    /// palette, not just the local buffer).
    #[test]
    fn update_skin_palette_applies_look_at_bone_delta_to_head() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("head".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let mut model =
            single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let frame = VrmaFrame::default();
        let look_at = LookAtBoneOutput {
            head: LookAtBoneDelta {
                delta: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            },
            left_eye: LookAtBoneDelta::default(),
            right_eye: LookAtBoneDelta::default(),
        };
        let palette = model.update_skin_palette(&frame, Some(&look_at));
        assert_eq!(palette.len(), 1);
        let transformed = palette[0].transform_point3(Vec3::new(1.0, 0.0, 0.0));
        assert!(
            (transformed - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "expected +X to rotate to -Z via LookAt, got {transformed:?}"
        );
        // The local rotation must be `rest * delta = delta`
        // (rest is identity in this fixture).
        assert!(
            (model.nodes.local_rotations[0]
                .dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)))
            .abs()
                > 1.0 - 1e-5,
            "head local rotation must be `rest * delta` after LookAt"
        );
    }

    /// The `LookAt` step runs **after** the VRMA step and
    /// multiplies the delta onto the current local rotation, so
    /// a frame that animates `head` plus an active `LookAt`
    /// delta ends up with `vrma * look_at` (additive tracking).
    #[test]
    fn update_skin_palette_look_at_composes_onto_vrma_for_head() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("head".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let mut model =
            single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let mut frame = VrmaFrame::default();
        let vrma = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        frame.bone_rotations.insert("head".into(), vrma);
        let look_delta = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let look_at = LookAtBoneOutput {
            head: LookAtBoneDelta { delta: look_delta },
            left_eye: LookAtBoneDelta::default(),
            right_eye: LookAtBoneDelta::default(),
        };
        let _palette = model.update_skin_palette(&frame, Some(&look_at));
        let expected = vrma * look_delta;
        assert!(
            model.nodes.local_rotations[0].dot(expected).abs() > 1.0 - 1e-5,
            "head local must be vrma * look_at, got {:?} expected {expected:?}",
            model.nodes.local_rotations[0]
        );
    }

    /// Identity `LookAt` deltas must be a no-op (the
    /// implementation skips them to avoid an unnecessary
    /// write). A model whose humanoid
    /// registry has none of `head` / `leftEye` / `rightEye`
    /// must produce a palette identical to the
    /// `None`-`LookAt` case — the call must not panic on
    /// `by_name` returning `None`.
    #[test]
    fn update_skin_palette_look_at_idempotent_for_zero_delta_or_missing_bones() {
        let mut humanoid = HumanoidBoneRegistry::new();
        humanoid.insert(
            VrmBone("hips".into()),
            HumanoidBoneEntry {
                node: 0,
                joint: Some(0),
                rest: BoneRestTransform::default(),
            },
        );
        let model_lookat = single_joint_model(
            humanoid.clone(),
            single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO),
        );
        let model_none =
            single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let frame = VrmaFrame::default();
        // Identity deltas on bones the model does not have
        // — the step must be a no-op.
        let look_at = LookAtBoneOutput::default();
        let palette_lookat = {
            let mut m = model_lookat;
            m.update_skin_palette(&frame, Some(&look_at)).to_vec()
        };
        let palette_none = {
            let mut m = model_none;
            m.update_skin_palette(&frame, None).to_vec()
        };
        assert_eq!(palette_lookat, palette_none);
    }

    /// Non-identity `LookAt` deltas on bones that the
    /// humanoid registry does **not** contain (e.g. a
    /// legacy VRM 0.x model without humanoid metadata,
    /// or a model whose author left the head/eye bones
    /// out of the skin) must be silently dropped. The
    /// palette must equal the rest-pose result, no
    /// `index out of bounds` panic.
    #[test]
    fn update_skin_palette_look_at_ignores_missing_humanoid_bones() {
        let humanoid = HumanoidBoneRegistry::new();
        let mut model =
            single_joint_model(humanoid, single_node_hierarchy(Quat::IDENTITY, Vec3::ZERO));
        let frame = VrmaFrame::default();
        let look_at = LookAtBoneOutput {
            head: LookAtBoneDelta {
                delta: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            },
            left_eye: LookAtBoneDelta {
                delta: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            },
            right_eye: LookAtBoneDelta {
                delta: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            },
        };
        let palette = model.update_skin_palette(&frame, Some(&look_at));
        assert_eq!(palette.len(), 1);
        assert_eq!(palette[0], Mat4::IDENTITY);
        // No panic, no partial write — the model's local
        // rotation buffer keeps its rest value.
        assert_eq!(model.nodes.local_rotations[0], Quat::IDENTITY);
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkinMatrix(pub [[f32; 4]; 4]);

impl From<Mat4> for SkinMatrix {
    fn from(m: Mat4) -> Self {
        Self(m.to_cols_array_2d())
    }
}

impl SkinMatrix {
    /// Size in bytes of one matrix — used by the loader to size the
    /// storage buffer.
    pub const SIZE: NonZeroU64 = NonZeroU64::new(64).expect("const: 64 is non-zero");
}
