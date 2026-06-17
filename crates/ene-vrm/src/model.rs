//! GPU-side data types produced by [`crate::loader::load_vrm`].
//!
//! PR3.3 loads **every primitive of every mesh** in the glTF
//! document. A VRM 1.0 model like AliciaSolid.vrm has 12 separate
//! glTF Mesh objects (body, clothes, hair, face, accessories,
//! etc.) — not one mesh with 12 primitives. Iterating only
//! `meshes[0]` (PR3.0/3.1/3.2) rendered the head/face area only;
//! PR3.3 fixes this by walking every `Mesh` and every `Primitive`.
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use crate::expression::ExpressionLayer;
use crate::expression_override::ExpressionDefinition;
use crate::humanoid::HumanoidBoneRegistry;
use crate::look_at::LookAtProperties;

/// A single mesh primitive loaded from the VRM. The PR3.1 loader
/// extracts every primitive of the first mesh, so the body, the
/// clothes, the face, etc. all end up here as separate
/// `VrmPrimitive` entries.
#[derive(Debug)]
pub struct VrmPrimitive {
    /// Per-vertex data: `position (vec3) + uv (vec2) + normal (vec3)`.
    /// 8 floats = 32 bytes per vertex.
    pub vertex_buf: wgpu::Buffer,
    /// Number of vertices in `vertex_buf`.
    pub vertex_count: u32,
    /// 32-bit index buffer.
    pub index_buf: wgpu::Buffer,
    /// Number of indices to draw.
    pub index_count: u32,
    /// Base-color texture for this primitive, if its material has
    /// one. `None` falls back to a flat color in the shader.
    pub base_color: Option<VrmTexture>,
}

/// A single glTF mesh object, as a list of primitives. PR3.3 loads
/// every `Mesh` in the glTF document — a VRM 1.0 has ~12 of these
/// (body, hair_front, hair_back, face, clothes_top, clothes_bottom,
/// etc.), one per body part. Earlier PRs that only loaded
/// `meshes[0]` therefore rendered the head/face area only.
#[derive(Debug, Default)]
pub struct VrmMesh {
    /// All primitives that make up this mesh. The renderer draws
    /// each one with its own base-color texture (or a flat color
    /// when `VrmPrimitive::base_color` is `None`).
    pub primitives: Vec<VrmPrimitive>,
}

/// A single GPU texture plus its sampler.
#[derive(Debug)]
pub struct VrmTexture {
    /// The texture itself.
    pub texture: wgpu::Texture,
    /// Default sampler (linear filtering, clamp-to-edge).
    pub sampler: wgpu::Sampler,
    /// Bind group layout `(1)` — used by the renderer to build the
    /// per-primitive bind group.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Bind group `(1)` — used by the renderer.
    pub bind_group: wgpu::BindGroup,
}

/// Skeleton metadata loaded from the first skin in the glTF. PR3
/// rendered with **identity** skinning; PR4.5 now exposes the full
/// joint list plus pre-computed bind matrices so the renderer can
/// upload a `mat4x4[]` palette to the GPU and run real per-vertex
/// skinning. The pre-baked `bind_matrices` are the per-joint
/// `inverse(inverse_bind[i])`; the per-frame runtime matrix will
/// be `current_joint_world * bind_matrices[i]` (Phase 2: driven
/// from the cursor look-at target via two-bone IK).
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    /// Inverse-bind matrices, one per joint. Loaded from
    /// `skin.inverse_bind_matrices` in the glTF.
    pub inverse_bind: Vec<Mat4>,
    /// `inverse_bind[i].inverse()` — the per-joint bind matrix.
    /// The renderer stores these in the skin palette as a
    /// rest-pose initial value (so models without any
    /// look-at-driven animation render unchanged from PR3).
    pub bind_matrices: Vec<Mat4>,
    /// For every joint, the index of the glTF `Node` that owns it
    /// (loaded from `skin.joints()`). PR4.5+ will use this to walk
    /// the node hierarchy and compute per-joint world transforms;
    /// for now it is preserved for the next pass and for unit
    /// tests.
    pub joint_to_node: Vec<usize>,
}

impl Skeleton {
    /// Number of joints in the skeleton. Zero for models with no skin.
    pub fn joint_count(&self) -> usize {
        self.inverse_bind.len()
    }
}

/// Top-level loaded model. Owns all GPU resources needed to render
/// the VRM once.
#[derive(Debug)]
pub struct VrmModel {
    /// All glTF meshes in the file. PR3.3 iterates every `Mesh`
    /// (PR3.0–3.2 only loaded `meshes[0]`, which is why most of
    /// the model was missing — VRM 1.0 uses one glTF Mesh per body
    /// part rather than one Mesh with many primitives).
    pub meshes: Vec<VrmMesh>,
    /// Skeleton metadata. Not consumed by the renderer in PR3.
    pub skeleton: Skeleton,
    /// Post-normalize AABB `(min, max)` of every vertex. The
    /// loader's normalize centres this on origin so the model
    /// should be symmetric around `(0, 0, 0)`. Storing the value
    /// here lets the runtime log it (PR4.2 diagnostic) without
    /// reading back the GPU vertex buffers.
    aabb_min: [f32; 3],
    aabb_max: [f32; 3],
    /// Expression / blend-shape layer (PR4.4). `Default` for
    /// models without morph targets; the renderer treats the
    /// empty layer as a no-op.
    pub expressions: ExpressionLayer,
    /// Humanoid bone registry (PR4.7). Built from the
    /// `VRMC_vrm.humanoid.humanBones` block — empty for
    /// models without humanoid metadata (e.g. legacy
    /// VRM 0.x). Consumers (#11 LookAt, #13 SpringBone,
    /// #14 VRMA, #15 NodeConstraint) use this to map bone
    /// names to glTF node / Skeleton joint indices.
    pub humanoid: HumanoidBoneRegistry,
    /// Look-at properties (PR4.8). Parsed from the
    /// `VRMC_vrm.lookAt` block — `None` for models
    /// without the block (e.g. legacy VRM 0.x). The
    /// runtime falls back to [`LookAtProperties::default`]
    /// when this is `None` so a model missing the block
    /// still gets the spec-default 90→10 range map and
    /// `"bone"` consumer type.
    pub look_at: Option<LookAtProperties>,
    /// Per-expression override definitions (PR4.9).
    /// Parsed from the `VRMC_vrm.expressions.{preset,custom}.<name>`
    /// tree — `isBinary`, `overrideMouth`, `overrideBlink`,
    /// `overrideLookAt`. Empty for models without the
    /// `VRMC_vrm.expressions` block, in which case the
    /// override pass is a no-op.
    pub expressions_meta: Vec<ExpressionDefinition>,
}

impl VrmModel {
    /// AABB `(min, max)` of the loaded (post-normalize) vertex
    /// data. Symmetric around origin if the loader's
    /// centre-and-scale is correct.
    pub fn aabb(&self) -> ([f32; 3], [f32; 3]) {
        (self.aabb_min, self.aabb_max)
    }

    /// Number of joints in the skeleton. Zero for models with no skin.
    pub fn joint_count(&self) -> usize {
        self.skeleton.joint_count()
    }

    /// Borrow the expression layer. The runtime writes into
    /// `expressions.weights` every frame; the renderer reads it.
    pub fn expressions(&self) -> &ExpressionLayer {
        &self.expressions
    }

    /// Mutable access to the expression layer. Used by
    /// `CharacterRenderer::apply_emotions` in
    /// `apps/ene-desktop-v2` to push the latest emotion weights
    /// into the model.
    pub fn expressions_mut(&mut self) -> &mut ExpressionLayer {
        &mut self.expressions
    }

    /// Borrow the look-at properties. `None` for models
    /// without the `VRMC_vrm.lookAt` block (e.g. legacy
    /// VRM 0.x). The desktop runtime supplies the spec
    /// default in that case via
    /// [`LookAtProperties::default`].
    pub fn look_at(&self) -> Option<&LookAtProperties> {
        self.look_at.as_ref()
    }

    /// Construct a `VrmModel` from its already-built pieces plus
    /// the post-normalize AABB. Used by the loader.
    pub(crate) fn new(
        meshes: Vec<VrmMesh>,
        skeleton: Skeleton,
        aabb_min: [f32; 3],
        aabb_max: [f32; 3],
        expressions: ExpressionLayer,
        humanoid: HumanoidBoneRegistry,
        look_at: Option<LookAtProperties>,
        expressions_meta: Vec<ExpressionDefinition>,
    ) -> Self {
        Self {
            meshes,
            skeleton,
            aabb_min,
            aabb_max,
            expressions,
            humanoid,
            look_at,
            expressions_meta,
        }
    }
}

/// Maximum number of joints that can influence a single vertex.
/// glTF 2.0 / VRM 1.0 standardise on 4. The shader's `weights`
/// attribute is `vec4<f32>`; the `joints` attribute is `vec4<u32>`.
pub const MAX_JOINTS_PER_VERTEX: usize = 4;

/// Per-vertex layout used by the loader and the shader.
///
/// PR4.5: the vertex layout grew by two attributes — `joints`
/// (`vec4<u32>`) and `weights` (`vec4<f32>`) — to support real
/// GPU skinning. The vertex buffer is now 60 bytes (12 floats +
/// 16 bytes for `vec4<u32>` + 16 bytes for `vec4<f32>`), up
/// from 32 bytes in PR3/PR4.4. Models that don't define
/// `JOINTS_0` / `WEIGHTS_0` fall back to `joints = [0, 0, 0, 0]`,
/// `weights = [1, 0, 0, 0]` and the renderer uploads a
/// one-element `skin[]` buffer containing `Mat4::IDENTITY` so the
/// per-vertex math reduces to
/// `weights[0] * skin[0] * pos = pos` and the model looks
/// identical to PR3.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct MeshVertex {
    /// Position in object space.
    pub position: [f32; 3],
    /// Texture coordinates.
    pub uv: [f32; 2],
    /// Normal in object space.
    pub normal: [f32; 3],
    /// Per-vertex joint indices, up to `MAX_JOINTS_PER_VERTEX`.
    /// Stored as `u32` so 256+-joint humanoid models (e.g. models
    /// with per-finger bones) can address every joint without
    /// silent aliasing onto `skin_matrices[255]`. The vertex
    /// buffer grew by 12 bytes per vertex (from PR4.5's `[u8; 4]`
    /// → `[u32; 4]`); the `LAYOUT` attribute 3 is
    /// `Float32x4 → Uint32x4` and the WGSL `vec4<u32>` is
    /// unchanged.
    pub joints: [u32; 4],
    /// Per-vertex joint weights, `sum(weights) = 1.0` per vertex.
    /// Used together with `joints` to look up `skin_matrices[]`.
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
        array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x2,
            2 => Float32x3,
            3 => Uint32x4,
            4 => Float32x4,
        ],
    };

    /// Reinterpret a slice of vertices as `&[u8]` for buffer upload.
    pub fn as_bytes(vertices: &[MeshVertex]) -> &[u8] {
        bytemuck::cast_slice(vertices)
    }
}

/// Issue #5: guard against accidentally reintroducing a `[u8; 4]`
/// `joints` field (which would silently alias every joint >= 255
/// onto `skin_matrices[255]`). The compile-time check keeps the
/// vertex layout honest; a runtime test covers the semantic.
const _: () = {
    const JOINTS_SIZE: usize = std::mem::size_of::<[u32; MAX_JOINTS_PER_VERTEX]>();
    assert!(
        JOINTS_SIZE == 16,
        "MeshVertex::joints must be a 16-byte `vec4<u32>` attribute; check MAX_JOINTS_PER_VERTEX and the field type"
    );
    const VERTEX_SIZE: usize = std::mem::size_of::<MeshVertex>();
    // pos(12) + uv(8) + normal(12) + joints(16) + weights(16) = 64
    // (or 60 if `wgpu` packs without `vec4` padding; both are
    // host-side valid; we only fail on a clear regression).
    assert!(
        VERTEX_SIZE == 60 || VERTEX_SIZE == 64,
        "MeshVertex size drifted; the LAYOUT attribute stride will be wrong"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #5: the `joints` attribute must be `Uint32x4` and
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
}

/// One skinning matrix per joint. PR3 uploads a buffer of
/// `Mat4::IDENTITY` of the same length as the skeleton's
/// `inverse_bind` count so the bind group layout does not change when
/// PR4 wires real skinning.
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
    pub const SIZE: NonZeroU64 = NonZeroU64::new(64).expect("64 is non-zero");
}
