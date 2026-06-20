// PR5.6: line-list debug overlay shader.
//
// Pairs of `(pos, color)` vertices form a single line
// segment. The vertex shader transforms `pos` by the
// camera's `view_proj`; the fragment shader emits the
// per-vertex color directly. The host picks the color
// (hit / idle / hit-point) so the same shader works for
// every kind of debug overlay.
struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

struct Camera {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
