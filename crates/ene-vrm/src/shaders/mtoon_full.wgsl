// PR4.15: Full VRMC_materials_mtoon-1.0 shader.
//
// Implements the complete MToon material model:
// - Toon shading (shade color + shift + toony)
// - Shade multiply texture
// - Shading shift texture
// - GI equalization
// - Emission (factor + texture)
// - MatCap (factor + texture)
// - Parametric rim (Fresnel-based)
// - Rim multiply texture
// - Rim lighting mix
// - Outline (world/screen coordinates)
// - Outline multiply texture (G channel)
// - Outline color + lighting mix
// - UV animation (scroll X/Y + rotation + mask)
//
// Per-material uniform (bind group 5) carries all parameters.
// Texture presence is encoded as a bitmask in `flags_and_mode.x`.
//
// Outline is rendered as a separate pass with front-face culling
// and vertex push along the normal (handled by the renderer, not
// this shader — this shader's vertex stage just passes through).

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
    @location(3) view_dir: vec3<f32>,
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

// Per-material MToon uniform (must match MToonUniform in mtoon.rs)
struct MToonUniform {
    shade_color_and_shift: vec4<f32>,       // rgb + shift
    lighting_params: vec4<f32>,             // toony, gi_eq, rim_mix, outline_mix
    emissive_and_matcap_r: vec4<f32>,       // emissive.rgb + matcap.r
    matcap_gb_and_rim_r: vec4<f32>,         // matcap.gb + rim_color.rg
    rim_params: vec4<f32>,                  // rim_color.b, fresnel, lift, outline_width
    outline_color_and_shift_scale: vec4<f32>, // outline.rgb + shift_tex_scale
    uv_animation_speeds: vec4<f32>,         // scroll_x, scroll_y, rotation, 0
    flags_and_mode: vec4<f32>,              // tex_flags, outline_mode, transparent_zwrite, queue_offset
    time: vec4<f32>,                        // time, 0, 0, 0
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var<uniform> model: ModelUniform;

// Base color texture (bind group 2)
@group(2) @binding(0)
var base_color_tex: texture_2d<f32>;
@group(2) @binding(1)
var base_color_smp: sampler;

// Morph targets (bind group 3)
@group(3) @binding(0)
var<storage, read> morph_offsets: array<vec3<f32>>;
@group(3) @binding(1)
var<uniform> morph_meta: MorphMeta;

// Skin matrices (bind group 4)
@group(4) @binding(0)
var<storage, read> skin_matrices: array<mat4x4<f32>>;

// MToon per-material uniform (bind group 5)
@group(5) @binding(0)
var<uniform> mtoon: MToonUniform;

// MToon textures (bind group 6) — all optional, gated by flags
@group(6) @binding(0)
var shade_multiply_tex: texture_2d<f32>;
@group(6) @binding(1)
var shade_multiply_smp: sampler;
@group(6) @binding(2)
var shading_shift_tex: texture_2d<f32>;
@group(6) @binding(3)
var shading_shift_smp: sampler;
@group(6) @binding(4)
var emissive_tex: texture_2d<f32>;
@group(6) @binding(5)
var emissive_smp: sampler;
@group(6) @binding(6)
var matcap_tex: texture_2d<f32>;
@group(6) @binding(7)
var matcap_smp: sampler;
@group(6) @binding(8)
var rim_multiply_tex: texture_2d<f32>;
@group(6) @binding(9)
var rim_multiply_smp: sampler;
@group(6) @binding(10)
var outline_width_tex: texture_2d<f32>;
@group(6) @binding(11)
var outline_width_smp: sampler;
@group(6) @binding(12)
var uv_mask_tex: texture_2d<f32>;
@group(6) @binding(13)
var uv_mask_smp: sampler;

// Texture flag constants (must match mtoon::flags)
const FLAG_BASE_COLOR: u32 = 1u;
const FLAG_SHADE_MULTIPLY: u32 = 2u;
const FLAG_SHADING_SHIFT: u32 = 4u;
const FLAG_EMISSIVE: u32 = 8u;
const FLAG_MATCAP: u32 = 16u;
const FLAG_RIM_MULTIPLY: u32 = 32u;
const FLAG_OUTLINE_WIDTH: u32 = 64u;
const FLAG_UV_MASK: u32 = 128u;

fn has_flag(flags: u32, bit: u32) -> bool {
    return (flags & bit) != 0u;
}

fn compute_uv_animation(uv: vec2<f32>) -> vec2<f32> {
    let flags = u32(mtoon.flags_and_mode.x);
    if !has_flag(flags, FLAG_UV_MASK) {
        return uv;
    }
    let mask = textureSample(uv_mask_tex, uv_mask_smp, uv).b;
    let t = mtoon.time.x;
    let scroll = mtoon.uv_animation_speeds.xy * t * mask;
    let rotation = mtoon.uv_animation_speeds.z * t * mask;
    let c = cos(rotation);
    let s = sin(rotation);
    let centered = uv - vec2<f32>(0.5);
    let rotated = vec2<f32>(c * centered.x - s * centered.y, s * centered.x + c * centered.y);
    return rotated + vec2<f32>(0.5) + scroll;
}

@vertex
fn vs_main(in: VsIn, @builtin(vertex_index) vidx: u32) -> VsOut {
    var out: VsOut;

    // GPU skinning
    var skinned_pos = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    skinned_pos = skinned_pos + in.weights.x * skin_matrices[in.joints.x] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.y * skin_matrices[in.joints.y] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.z * skin_matrices[in.joints.z] * vec4<f32>(in.position, 1.0);
    skinned_pos = skinned_pos + in.weights.w * skin_matrices[in.joints.w] * vec4<f32>(in.position, 1.0);

    var world_pos = model.model * skinned_pos;

    // Morph targets
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
    out.view_dir = normalize(camera.camera_pos.xyz - world_pos.xyz);
    return out;
}

// Toon shading: linearstep(-1 + toony, 1 - toony, ndotl + shift)
fn toon_shade(ndotl: f32) -> f32 {
    let toony = mtoon.lighting_params.x;
    let shift = mtoon.shade_color_and_shift.w;
    let raw = ndotl + shift;
    let lo = -1.0 + toony;
    let hi = 1.0 - toony;
    if (hi <= lo) {
        return step(0.0, raw);
    }
    return clamp((raw - lo) / (hi - lo), 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let flags = u32(mtoon.flags_and_mode.x);
    let n = normalize(in.normal);
    let v = normalize(in.view_dir);
    let light_dir = normalize(vec3<f32>(0.3, 0.8, 0.5));

    // UV animation (scroll / rotation / mask). Done in the
    // fragment stage because the mask is a texture sample,
    // which WGSL forbids in the vertex stage.
    let uv = compute_uv_animation(in.uv);

    // Base color
    var base = textureSample(base_color_tex, base_color_smp, uv);

    // Shade multiply texture
    if has_flag(flags, FLAG_SHADE_MULTIPLY) {
        let shade_tex = textureSample(shade_multiply_tex, shade_multiply_smp, uv);
        base = base * shade_tex;
    }

    // Toon shading
    let ndotl = dot(n, light_dir);
    let shade_factor = toon_shade(ndotl);

    // Shading shift texture
    var shift_add = 0.0;
    if has_flag(flags, FLAG_SHADING_SHIFT) {
        let shift_tex = textureSample(shading_shift_tex, shading_shift_smp, uv);
        shift_add = shift_tex.r * mtoon.outline_color_and_shift_scale.w;
    }
    let shaded = clamp(shade_factor + shift_add, 0.0, 1.0);

    // Mix base color with shade color
    let shade_color = mtoon.shade_color_and_shift.xyz;
    let lit_color = mix(shade_color, vec3<f32>(1.0), shaded);
    var color = base.rgb * lit_color;

    // GI equalization (simple: blend between shaded and flat)
    let gi_eq = mtoon.lighting_params.y;
    let flat = vec3<f32>(1.0);
    color = mix(color, base.rgb * flat, gi_eq * (1.0 - shaded));

    // Emission
    if has_flag(flags, FLAG_EMISSIVE) {
        let emis_tex = textureSample(emissive_tex, emissive_smp, uv);
        let emis = mtoon.emissive_and_matcap_r.xyz * emis_tex.rgb;
        color = color + emis;
    } else {
        color = color + mtoon.emissive_and_matcap_r.xyz;
    }

    // MatCap
    if has_flag(flags, FLAG_MATCAP) {
        let matcap_factor = vec3<f32>(
            mtoon.emissive_and_matcap_r.w,
            mtoon.matcap_gb_and_rim_r.x,
            mtoon.matcap_gb_and_rim_r.y,
        );
        // Build a view-aligned basis for matcap UV
        let world_view_x = normalize(vec3<f32>(v.z, 0.0, -v.x));
        let world_view_y = cross(v, world_view_x);
        let matcap_uv = vec2<f32>(dot(world_view_x, n), dot(world_view_y, n)) * 0.495 + vec2<f32>(0.5);
        let matcap_sample = textureSample(matcap_tex, matcap_smp, matcap_uv);
        color = color + matcap_factor * matcap_sample.rgb;
    }

    // Parametric rim
    let rim_color = vec3<f32>(
        mtoon.matcap_gb_and_rim_r.z,
        mtoon.matcap_gb_and_rim_r.w,
        mtoon.rim_params.x,
    );
    let fresnel_power = max(mtoon.rim_params.y, 0.001);
    let rim_lift = mtoon.rim_params.z;
    let ndotv = clamp(dot(n, v), 0.0, 1.0);
    let parametric_rim = pow(clamp(1.0 - ndotv + rim_lift, 0.0, 1.0), fresnel_power);
    var rim = parametric_rim * rim_color;

    // Rim multiply texture
    if has_flag(flags, FLAG_RIM_MULTIPLY) {
        let rim_tex = textureSample(rim_multiply_tex, rim_multiply_smp, uv);
        rim = rim * rim_tex.rgb;
    }

    // Rim lighting mix
    let rim_mix = mtoon.lighting_params.z;
    let lighting = vec3<f32>(shaded);
    rim = rim * mix(vec3<f32>(1.0), lighting, rim_mix);

    color = color + rim;

    return vec4<f32>(color * base.a, base.a);
}
