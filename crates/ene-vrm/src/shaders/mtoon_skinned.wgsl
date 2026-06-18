// PR4.5 skinned variant of `mtoon_lite.wgsl`.
//
// The vertex layout grows by two attributes (joints + weights) and
// a new bind group `(4)` carries the per-joint `skin_matrices`
// palette. The vertex shader computes:
//
//     skinned_pos = sum(weights[i] * skin_matrices[joints[i]] * pos)
//
// where `joints[i]` is the joint index, `weights[i]` is its
// influence, and `skin_matrices[i]` is the per-joint matrix
// uploaded by the renderer.
//
// At rest pose the renderer uploads `bind_matrices[i] =
// inverse_bind[i].inverse()`. The standard glTF skinning identity
// is then `weights[0] * bind_matrices[joints[0]] * inverse_bind[joints[0]] * pos
// = pos`, so a model with no animation is rendered unchanged
// from the PR3 / PR4.4 un-skinned path. Models that don't define
// `JOINTS_0` / `WEIGHTS_0` fall back to `joints = [0,0,0,0]` and
// `weights = [1,0,0,0]`; the renderer then uploads a
// one-element `skin[]` of `Mat4::IDENTITY` and the math
// collapses to `pos`.
//
// The fragment stage is identical to `mtoon_lite.wgsl` (lit
// half-Lambert + base-color sample); skinning is purely a
// vertex-stage transform. The full MToon material model (rim /
// matcap / outline / emission) ships in a follow-up PR.

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

@group(2) @binding(0)
var base_color_tex: texture_2d<f32>;
@group(2) @binding(1)
var base_color_smp: sampler;

@group(3) @binding(0)
var<storage, read> morph_offsets: array<vec3<f32>>;
@group(3) @binding(1)
var<uniform> morph_meta: MorphMeta;

// PR4.5: per-joint skin matrix palette. The renderer uploads
// `bind_matrices[i] = inverse_bind[i].inverse()` as the rest-pose
// initial value; Phase 2 will overwrite it every frame with
// `current_joint_world[i] * bind_matrices[i]`.
@group(4) @binding(0)
var<storage, read> skin_matrices: array<mat4x4<f32>>;

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vidx: u32) -> VsOut {
    var out: VsOut;

    // PR4.5: GPU skinning. Iterate the four (joint, weight) pairs
    // and accumulate the weighted skin-matrix product. The
    // `weights[0] == 1.0 && rest == 0.0` case collapses to
    // `skin_matrices[joints[0]] * pos` (the standard "single joint
    // influences this vertex" form). PR4.5 ships a rest-pose
    // palette only — Phase 2 will drive the matrices from the
    // cursor look-at target.
    var skinned_pos = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    skinned_pos = skinned_pos + in.weights.x * skin_matrices[in.joints.x] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.y * skin_matrices[in.joints.y] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.z * skin_matrices[in.joints.z] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.w * skin_matrices[in.joints.w] * vec4<f32>(in.position, 1.0);

    var world_pos = model.model * skinned_pos;

    // PR4.4: accumulate morph-target offsets. The `target_count`
    // gate keeps the cost near zero on primitives that do not
    // define morph targets (their bind group uses a dummy layout
    // with `target_count = 0u`). The bound storage buffer is
    // always at least one vec3 wide, so the array indexing is
    // valid as long as we never enter the loop.
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
    let light_dir = normalize(vec3<f32>(0.3, 0.8, 0.5));
    let n = normalize(in.normal);
    let ndotl = clamp(dot(n, light_dir), 0.0, 1.0);
    let half_lambert = pow(ndotl * 0.5 + 0.5, 2.0);
    let lit = vec3<f32>(0.4) + vec3<f32>(0.6) * vec3<f32>(half_lambert);
    let color = base.rgb * lit;
    return vec4<f32>(color * base.a, base.a);
}
