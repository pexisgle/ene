// Issue #19: unlit variant of `mtoon_skinned.wgsl`.
//
// KHR_materials_unlit materials skip all lighting calculations
// and output the base color directly. Used for emissive panels,
// flat signs, comic-style eye highlights, etc.
//
// The vertex stage is identical to `mtoon_skinned.wgsl` (GPU
// skinning + morph targets). The fragment stage samples the base
// color and outputs it directly without any half-Lambert term.

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) normal: vec3<f32>,
};

struct CameraUniform {
    view_proj: mat4x4<f32>,
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

@group(2) @binding(0)
var base_color_tex: texture_2d<f32>;
@group(2) @binding(1)
var base_color_smp: sampler;

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
    out.uv = in.uv;
    out.world_pos = world_pos.xyz;
    out.normal = (model.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(base_color_tex, base_color_smp, in.uv);
    return vec4<f32>(base.rgb * base.a, base.a);
}
