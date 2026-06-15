// PR3 lit-lite shader. Vertex transforms the position by the
// camera's view-projection matrix and forwards the UV / normal to
// the fragment stage. Fragment samples the base-color texture (if
// present in bind group `(2)`) and applies a half-Lambert diffuse
// term against a single fixed directional light.
//
// PR4.1: the per-frame `model` uniform (bind group 1) is applied
// between view-proj and the vertex position. The runtime composes
// it from `CharacterState::character_position` + `model_scale`.
//
// PR4.4: bind group `(3)` carries the morph-target data for
// primitives that have blend shapes. `morph_offsets` is a
// `storage<read>` array of `vec3<f32>` laid out
// `[target_idx * vertex_count + vertex_idx]`; `morph_meta` is
// the per-primitive [`PrimitiveMorphMeta`] uniform (see
// `expression.rs`). The vertex shader early-outs when
// `target_count == 0u` so primitives without morphs pay no cost.
//
// Outline / rim / matcap / emission / shading-shift all live in
// follow-up PRs (the full MToon material model).

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
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

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vidx: u32) -> VsOut {
    var out: VsOut;
    var world_pos = model.model * vec4<f32>(in.position, 1.0);

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
    // PR4.x will switch the lighting term to use `world_pos` so
    // the half-Lambert light direction stays fixed in world space
    // even when the model translates. For now we just transform the
    // normal through the (rotation-only) model matrix; scale
    // uniform so no inverse-transpose is required.
    out.normal = (model.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(base_color_tex, base_color_smp, in.uv);
    // PR3 lit model: a single directional light from the +Y +X
    // direction, half-Lambert wrap so the back of the model is not
    // pitch black. MToon's "shading shift" knob ships in a follow-up
    // PR.
    let light_dir = normalize(vec3<f32>(0.3, 0.8, 0.5));
    let n = normalize(in.normal);
    let ndotl = clamp(dot(n, light_dir), 0.0, 1.0);
    let half_lambert = pow(ndotl * 0.5 + 0.5, 2.0);
    let lit = vec3<f32>(0.4) + vec3<f32>(0.6) * vec3<f32>(half_lambert);
    let color = base.rgb * lit;
    return vec4<f32>(color * base.a, base.a);
}
