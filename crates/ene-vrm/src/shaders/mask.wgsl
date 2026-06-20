//! PR-LX.7: Solid-color mask shader for the offscreen Wayland/X11
//! silhouette capture.
//!
//! Mirrors the character camera (`view_proj` + `model`) but writes
//! a single opaque white fragment for every covered pixel. The
//! rectangle extractor in `wayland_mask_capture::extract_rectangles`
//! reads the red channel of the captured `Rgba8Unorm` target and
//! flags `r > PIXEL_THRESHOLD` as "inside the silhouette".
//!
//! Bind group layout:
//! - `(0)` — [`CameraUniform`]: `view_proj` (mat4x4f).
//! - `(1)` — [`ModelUniform`]: `model` (mat4x4f).
//!
//! Vertex input:
//! - attribute 0: `position: vec3<f32>`.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
};

struct ModelUniform {
    model: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: CameraUniform;

@group(1) @binding(0)
var<uniform> model_uniform: ModelUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
    let world = model_uniform.model * vec4<f32>(input.position, 1.0);
    var out: VsOut;
    out.clip_pos = camera.view_proj * world;
    return out;
}

@fragment
fn fs_main(_input: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
