# Stage UI PoC findings

Tracking: [#1260](https://github.com/pexisgle/ene/issues/1260), probes [#1258](https://github.com/pexisgle/ene/pull/1258).
Japanese: [ja/experiments/stage-ui-poc-findings.md](../ja/experiments/stage-ui-poc-findings.md).

The raw `ene-stage-poc` crate stays off `main`. This page is the decision log
for Stage v2. Later issues treat the Adopt rows as constraints, not as topics
to reopen.

## Experiments

| Probe | Result | Decision |
|---|---|---|
| A/B shared wgpu + Slint composition | Same `Instance` / `Device` / `Queue`. In-process UI → VRM → passthrough routing unit-tested. | Adopt |
| C compositor cost (Vulkan, 800×600, release) | Fullscreen premul pass, `LoadOp::Load`. No CPU readback, no `copy_texture_to_texture`. Idle is 0 frames / 0 CPU. Slint overhead acceptable on the probe GPU. | Adopt GPU-resident composition |
| D Wayland input region (Weston 13 nested/native) | `wl_surface::set_input_region` delivers in-region events and does not deliver empty-glass events. Hover-rearm not required. | Adopt on native Wayland. Not claimed for every compositor. |
| D2 X11 SHAPE (xfwm4, compositing on) | Input-only SHAPE resets. Bounding > Input simple rects stick. Complex Bounding unions may expand to the full client. Input is YX-banded. Pixels outside Bounding are clipped. ShapeNotify reapply fights the WM (~151 SET/s). | Adopt coarse Bounding + Input. Reject ShapeNotify reapply. Reject pixel-perfect silhouette. |

Software rasterizer numbers from a machine without `/dev/dri` are **not** a
production performance gate. Record them as software-reference only.

## Adopt

- Slint + shared wgpu 29. Isolate `unstable-wgpu-29` at the renderer boundary.
- Slint draws to an offscreen `Rgba8Unorm` target.
- Composite with a fullscreen triangle, `PREMULTIPLIED_ALPHA_BLENDING`, `LoadOp::Load`.
- No CPU readback presentation.
- Idle: no continuous redraw when nothing changed.
- Wayland: `InteractionGeometry` → `wl_surface::set_input_region`. Dirty + few-px threshold + rate limit (~8 Hz while moving). No hover-rearm.
- X11: `VisualGeometry` → coarse Bounding (few AABB + effect padding). `InteractionGeometry` → coarse Input. Runtime sanity check and window-wide fallback.
- Windows: keep DX12 / DirectComposition / wgpu StageWindow and window-wide `Window::set_cursor_hittest()`. Explicit `Passive / Interactive / Dragging / UiFocused` lifecycle first.
- VRM body-part hit-test is independent of window architecture. First candidate: bone-derived CPU colliders / coarse screen-space regions.

## Reject

- GPU readback presentation.
- Windows layered full-frame presentation, helper HWND, small/tight-window architecture.
- Windows cross-process partial click-through.
- X11 Input-only SHAPE as the default.
- X11 ShapeNotify reapply.
- X11 pixel-perfect visual/input silhouette.
- `override_redirect` as a Stage default.
- Merging `crates/ene-stage-poc` into production.
- Starting Chat / Detail Slint ports before the Stage production gate (#1273) is stable.

## Regression checklist

Later PRs compare against [stage-v2-baseline.md](stage-v2-baseline.md):

- Transparency (premultiplied swapchain; hide overlay when unsupported)
- Always-on-top
- Platform input (Windows window-wide hittest, Wayland input region, X11 SHAPE + fallback)
- VRM hover / click / drag
- Display-only UI stays Passive; clickable UI can take the first click
- DPI / resize
- Multiple avatars
- Idle redraw stops when unchanged
