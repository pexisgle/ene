# Stage UI PoC findings

This document preserves the architecture findings from draft PR #1258 without merging the one-off `ene-stage-poc` implementation into the main workspace.

Source snapshot: `58421613f413df858c2ec389e4772cf1ee665e08` from #1258.

## Scope

The probe evaluated a future Stage path that combines the existing VRM/wgpu renderer with Slint UI in one transparent native window, plus platform input-region behavior. It was a technical spike, not a production implementation.

## Adopted rendering findings

- Slint can share the same wgpu 29 `Instance` / `Device` / `Queue` used by the VRM renderer.
- The validated composition path is GPU-resident:
  1. render VRM/3D,
  2. render Slint FemtoVG to an offscreen `Rgba8Unorm` target,
  3. composite that target with a fullscreen premultiplied-alpha pass using `LoadOp::Load`,
  4. present the existing Stage surface.
- The composition path requires no GPU-to-CPU readback and does not rely on `copy_texture_to_texture`.
- Slint FemtoVG clears its target, so an offscreen UI target plus a compositor pass is required rather than drawing Slint directly into the existing 3D pass.
- `unstable-wgpu-29` is an upgrade/maintenance risk, not a demonstrated runtime blocker. Keep this dependency isolated behind the Stage renderer boundary.

## Performance evidence

Experiment C compared equivalent release-build paths on Linux Vulkan with llvmpipe after warm-up. These numbers are useful for relative cost only; they are not a real-GPU acceptance benchmark.

| Delta / path | Result |
|---|---:|
| Slint init + offscreen target + premultiplied compositor (`C1 - C0`) | +2.79 ms average frame |
| Static Slint bubble (`C2 - C1`) | +0.68 ms |
| Light Slint animation (`C3 - C2`) | +0.05 ms |
| Slint offscreen path (`C2`) | 5.44 ms average frame |
| egui Load-on-swapchain comparison (`C4`) | 2.08 ms average frame |

The idle probe produced 0 continuous frames / approximately 0 CPU once unchanged. The PoC therefore did not identify Slint redraw scheduling as a blocker, but production acceptance still requires release measurements on real GPUs.

## In-process interaction model

The validated routing order is:

```text
UI -> VRM -> passthrough
```

UI wins when UI and VRM interaction geometry overlap. VRM interaction can use coarse CPU geometry; the PoC did not establish a need for continuous GPU picking/readback.

Visual geometry and interaction geometry must remain separate concepts. Platform APIs should consume interaction/visual projections without leaking platform-specific policy into Slint components.

## Wayland findings

On a native Weston 13 test session, `wl_surface::set_input_region` worked as intended:

- UI / VRM regions received pointer input.
- Empty-glass regions were not delivered to the Stage surface and could pass through to the surface behind it.
- No hover-rearm design is required when the maintained OS region is the union of current interactive geometry.
- Updating the region every frame is unnecessary; dirty tracking plus throttling is sufficient while geometry moves.

This result is evidence for Weston 13, not a guarantee for every Wayland compositor. Production code must retain compositor-aware diagnostics/fallback policy.

## X11 findings

The xfwm4 experiments showed that SHAPE behavior is WM-dependent:

- Input-only SHAPE was reset to the full client region after a short delay on the tested xfwm4 setup.
- Setting Bounding and Input together can remain effective for simple shapes.
- A simple `Bounding > Input` split allowed interaction inside the Input region while clicks outside it reached a separate click-sink process.
- Complex Bounding unions may be expanded by the WM to the full client, while Input can remain smaller / YX-banded.
- Pixels outside Bounding are clipped, so visual effects need a coarse Bounding region with appropriate padding.
- Reapplying on every `ShapeNotify` caused an approximately 151 updates/s fight with the WM and is explicitly rejected.

Production X11 therefore uses coarse SHAPE/fallback behavior, not pixel-perfect or continuously reasserted regions.

## Windows finding and rejected evidence

The original Experiment B implementation used `SetWindowRgn` as a partial-region probe. That result must **not** be treated as evidence for input-only partial click-through: `SetWindowRgn` changes the window shape/visible region, so it does not establish that the full rendered Stage surface can remain visible while only selected areas receive cross-process input.

Adopted Windows policy is therefore:

- keep the existing DX12 / DirectComposition / wgpu Stage window architecture;
- keep window-wide cursor hit testing (`set_cursor_hittest()` or equivalent) under `StageInteractionController`;
- use explicit `Passive`, `Interactive`, `Dragging`, and `UiFocused` lifecycle states;
- do not pursue layered/helper/small-window or partial cross-process click-through as part of Stage v2.

The Windows partial-region claim in the raw #1258 branch is superseded by this conclusion and by the Stage v2 architecture in #1260/#1268.

## Architecture decisions carried into Stage v2

The PoC supports the following production decisions:

- shared-wgpu VRM + Slint composition is viable;
- UI composition remains GPU-resident with premultiplied alpha;
- idle must not continuously redraw;
- UI hit testing takes priority over VRM hit testing;
- `VisualGeometry` and `InteractionGeometry` stay independent;
- Wayland projects interaction geometry to input regions;
- X11 uses coarse Bounding/Input SHAPE with fallback and no WM fight;
- Windows retains the current DComp/wgpu window and window-wide interaction mode;
- platform policy stays outside Slint component code.

## What was intentionally not merged

The `crates/ene-stage-poc` code from #1258 is not part of the production workspace. If a probe becomes useful as a regression/diagnostic test, reintroduce only that focused probe with a maintained purpose and acceptance criterion.

High-value candidates for future diagnostic coverage are:

- shared-wgpu compositor benchmark/sanity probe;
- Wayland input-region sanity probe;
- X11 SHAPE + click-sink sanity probe;
- Windows current-architecture interaction regression probe.

Production implementation and final cross-platform acceptance remain tracked under #1260 and its child issues.
