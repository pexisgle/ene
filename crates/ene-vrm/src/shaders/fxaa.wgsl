// FXAA Quality preset (medium quality, fast path).
// Based on the public-domain FXAA implementation by Timothy Lottes
// (https://www.shadertoy.com/view/ls3GWN), trimmed to the inputs
// the v2 renderer already provides (the source texture + the
// inverse texture size).
//
// Notes for the v2 port:
//   * The shader uses luma-based edge detection on RGB; an
//     explicit `luma` channel is not required.
//   * The two sliders (subpixel quality, edge threshold) are
//     hard-coded; a future config-driven variant can re-introduce
//     them.
//   * Premultiplied-alpha output matches the rest of the
//     renderer (the v2 pipeline writes `vec4(rgb * a, a)` for
//     transparency, and FXAA must preserve that contract).
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct FsOut {
    @location(0) color: vec4<f32>,
};

struct PostUniforms {
    inv_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var src_tex: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;
@group(0) @binding(2) var<uniform> uniforms: PostUniforms;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>) -> VsOut {
    var out: VsOut;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    return out;
}

const EDGE_THRESHOLD: f32 = 0.166;
const EDGE_THRESHOLD_MIN: f32 = 0.0833;
const SUBPIXEL_QUALITY: f32 = 0.75;

fn sample_luma(uv: vec2<f32>) -> f32 {
    let c = textureSample(src_tex, src_sampler, uv);
    return dot(c.rgb, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let inv = uniforms.inv_size;
    let uv = in.uv;

    let luma_center = sample_luma(uv);
    let luma_n = sample_luma(uv + vec2<f32>(0.0, -inv.y));
    let luma_s = sample_luma(uv + vec2<f32>(0.0, inv.y));
    let luma_e = sample_luma(uv + vec2<f32>(inv.x, 0.0));
    let luma_w = sample_luma(uv + vec2<f32>(-inv.x, 0.0));

    let luma_min = min(luma_center, min(min(luma_n, luma_s), min(luma_e, luma_w)));
    let luma_max = max(luma_center, max(max(luma_n, luma_s), max(luma_e, luma_w)));
    let luma_range = luma_max - luma_min;

    let range_max = max(EDGE_THRESHOLD, EDGE_THRESHOLD_MIN);
    if luma_range < range_max {
        var out: FsOut;
        out.color = textureSample(src_tex, src_sampler, uv);
        return out;
    }

    let luma_ne = sample_luma(uv + vec2<f32>(inv.x, -inv.y));
    let luma_nw = sample_luma(uv + vec2<f32>(-inv.x, -inv.y));
    let luma_se = sample_luma(uv + vec2<f32>(inv.x, inv.y));
    let luma_sw = sample_luma(uv + vec2<f32>(-inv.x, inv.y));

    let luma_down_sum = luma_n + luma_s + luma_se + luma_sw;
    let luma_up_sum = luma_n + luma_nw + luma_w + luma_e;
    let luma_left_sum = luma_nw + luma_w + luma_sw + luma_s;
    let luma_right_sum = luma_ne + luma_e + luma_se + luma_s;

    let luma_down = luma_down_sum * 0.25;
    let luma_up = luma_up_sum * 0.25;
    let luma_left = luma_left_sum * 0.25;
    let luma_right = luma_right_sum * 0.25;

    let luma_min_horiz = min(luma_left, luma_right);
    let luma_max_horiz = max(luma_left, luma_right);
    let luma_min_vert = min(luma_down, luma_up);
    let luma_max_vert = max(luma_down, luma_up);

    let ratio_horiz = (luma_max_horiz - luma_min_horiz) / luma_range;
    let ratio_vert = (luma_max_vert - luma_min_vert) / luma_range;

    var step: vec2<f32>;
    if ratio_horiz >= ratio_vert {
        step = vec2<f32>(inv.x, 0.0);
        if luma_left < luma_right {
            step = -step;
        }
    } else {
        step = vec2<f32>(0.0, inv.y);
        if luma_down < luma_up {
            step = -step;
        }
    }

    let luma_local_avg = (luma_n + luma_s + luma_e + luma_w) * 0.25;
    let subpix = clamp(
        abs(luma_local_avg - luma_center) / luma_range,
        0.0,
        1.0,
    );
    let subpix_offset = (-2.0 * subpix + 3.0) * subpix * subpix * SUBPIXEL_QUALITY;

    let uv1 = uv + step * (1.0 - subpix_offset);
    let uv2 = uv + step * (2.0 - subpix_offset * 0.5);
    let luma_end1 = sample_luma(uv1);
    let luma_end2 = sample_luma(uv2);
    let luma_final = (luma_center + luma_end1 + luma_end2) * (1.0 / 3.0);

    var out: FsOut;
    if abs(luma_final - luma_local_avg) < max(EDGE_THRESHOLD_MIN, luma_range * 0.5) {
        out.color = textureSample(src_tex, src_sampler, uv);
    } else {
        out.color = textureSample(src_tex, src_sampler, uv1);
    }
    return out;
}
