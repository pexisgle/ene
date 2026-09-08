//! Solid-color mask shader for the offscreen Wayland/X11
//! silhouette capture.
//!
//! Mirrors the character camera (`view_proj` + `model`) but writes
//! a single opaque white fragment for every covered pixel. The
//! rectangle extractor in `wayland_mask_capture::extract_rectangles`
//! reads the red channel of the captured `Rgba8Unorm` target and
//! flags `r > PIXEL_THRESHOLD` as "inside the silhouette".
//!
//! Bind group layout (matches other skinned shaders):
//! - `(0)` — [`CameraUniform`]: `view_proj` (mat4x4f).
//! - `(1)` — [`ModelUniform`]: `model` (mat4x4f).
//! - `(3)` — Morph target offsets + meta.
//! - `(4)` — Skin matrix palette.
//!
//! Vertex input:
//! - attribute 0: `position: vec3<f32>`.
//! - attribute 3: `joints: vec4<u32>`.
//! - attribute 4: `weights: vec4<f32>`.

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
};

struct ModelUniform {
    model: mat4x4<f32>,
};

const MAX_WEIGHT_SLOTS: u32 = 16u;

struct MorphMeta {
    vertex_count: u32,
    target_count: u32,
    _pad0: u32,
    _pad1: u32,
    weights: array<vec4<f32>, MAX_WEIGHT_SLOTS>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var<uniform> model: ModelUniform;

@group(3) @binding(0)
var<storage, read> morph_offsets: array<vec3<f32>>;
@group(3) @binding(1)
var<uniform> morph_meta: MorphMeta;

@group(4) @binding(0)
var<storage, read> skin_matrices: array<mat4x4<f32>>;

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vidx: u32) -> VsOut {
    var out: VsOut;

    var skinned_pos = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    skinned_pos = skinned_pos + in.weights.x * skin_matrices[in.joints.x] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.y * skin_matrices[in.joints.y] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.z * skin_matrices[in.joints.z] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.w * skin_matrices[in.joints.w] * vec4<f32>(in.position, 1.0);

    var world_pos = model.model * skinned_pos;

    if (morph_meta.target_count > 0u) {
        var morph_delta = vec3<f32>(0.0);
        for (var t: u32 = 0u; t < morph_meta.target_count; t = t + 1u) {
            let slot = t / 4u;
            let comp = t % 4u;
            let w = morph_meta.weights[slot][comp];
            if (w != 0.0) {
                let offset = morph_offsets[t * morph_meta.vertex_count + vidx];
                morph_delta = morph_delta + offset * w;
            }
        }
        world_pos = world_pos + vec4<f32>(morph_delta, 0.0);
    }

    out.clip_pos = camera.view_proj * world_pos;
    return out;
}

@fragment
fn fs_main(_input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
