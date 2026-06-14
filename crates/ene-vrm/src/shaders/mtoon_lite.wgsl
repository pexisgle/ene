// PR3 lit-lite shader. Vertex transforms the position by the
// camera's view-projection matrix and forwards the UV / normal to
// the fragment stage. Fragment samples the base-color texture (if
// present in bind group `(1)`) and applies a half-Lambert diffuse
// term against a single fixed directional light.
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
    @location(1) normal: vec3<f32>,
};

struct CameraUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var base_color_tex: texture_2d<f32>;
@group(1) @binding(1)
var base_color_smp: sampler;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = camera.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.normal = in.normal;
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
    let half_lambert = pow(ndotl * 0.5 + 0.5, vec2<f32>(2.0));
    let lit = vec3<f32>(0.4) + vec3<f32>(0.6) * half_lambert;
    let color = base.rgb * lit;
    return vec4<f32>(color * base.a, base.a);
}
