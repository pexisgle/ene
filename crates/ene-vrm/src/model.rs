//! GPU-side data types produced by [`crate::loader::load_vrm`].
//!
//! PR3 ships a single mesh + single texture. Multi-mesh / multi-material
//! support lands in a follow-up PR.
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

/// A single mesh primitive loaded from the VRM. The PR3 loader
/// extracts the first primitive of the first mesh only.
#[derive(Debug)]
pub struct VrmMesh {
    /// Per-vertex data: `position (vec3) + uv (vec2) + normal (vec3)`.
    /// 8 floats = 32 bytes per vertex.
    pub vertex_buf: wgpu::Buffer,
    /// Number of vertices in `vertex_buf`.
    pub vertex_count: u32,
    /// 32-bit index buffer.
    pub index_buf: wgpu::Buffer,
    /// Number of indices to draw.
    pub index_count: u32,
}

/// A single GPU texture plus its sampler.
#[derive(Debug)]
pub struct VrmTexture {
    /// The texture itself.
    pub texture: wgpu::Texture,
    /// Default sampler (linear filtering, clamp-to-edge).
    pub sampler: wgpu::Sampler,
    /// Bind group layout `(2)` — used by the renderer to build the
    /// per-model bind group.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Bind group `(2)` — used by the renderer.
    pub bind_group: wgpu::BindGroup,
}

/// Skeleton metadata loaded from the first skin in the glTF. PR3
/// renders with **identity** skinning (every joint transform is
/// `Mat4::IDENTITY`); the joint math ships in PR4.
#[derive(Debug, Clone, Default)]
pub struct Skeleton {
    /// Inverse-bind matrices, one per joint. PR3 does not use them
    /// for rendering but stores them so PR4 can plug in skinning
    /// without re-parsing the file.
    pub inverse_bind: Vec<Mat4>,
}

/// Top-level loaded model. Owns all GPU resources needed to render
/// the VRM once.
#[derive(Debug)]
pub struct VrmModel {
    /// Vertex + index buffers for the first primitive.
    pub mesh: VrmMesh,
    /// Base-color texture (or `None` if the material has no base
    /// texture — the shader falls back to a flat color).
    pub base_color: Option<VrmTexture>,
    /// Skeleton metadata. Not consumed by the renderer in PR3.
    pub skeleton: Skeleton,
}

/// Per-vertex layout used by the loader and the shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct MeshVertex {
    /// Position in object space.
    pub position: [f32; 3],
    /// Texture coordinates.
    pub uv: [f32; 2],
    /// Normal in object space.
    pub normal: [f32; 3],
}

impl MeshVertex {
    /// Vertex buffer layout entry shared by the loader and the
    /// renderer.
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x2,
            2 => Float32x3,
        ],
    };

    /// Reinterpret a slice of vertices as `&[u8]` for buffer upload.
    pub fn as_bytes(vertices: &[MeshVertex]) -> &[u8] {
        bytemuck::cast_slice(vertices)
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
