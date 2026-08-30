# Stage UI probe: Slint + shared wgpu

Isolated technical probes for a future Stage window that composites
3D (wgpu / `ene-vrm`) and 2D UI (Slint) in one transparent native
window. **Not a product path.** Production `ene-stage` still uses
egui chrome + a wgpu overlay and is unchanged.

Code: `crates/ene-stage-poc/`

| Binary | Experiment |
|---|---|
| `ene-stage-poc-a` | A: shared wgpu composition |
| `ene-stage-poc-b` | B: UI → VRM → passthrough routing |
| `ene-stage-poc-baseline` | egui + triangle on the same `GpuContext` |

```sh
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-a
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-b
ENE_STAGE_POC_SECONDS=8 cargo run -p ene-stage-poc --bin ene-stage-poc-baseline
```

`Esc` or `q` quits. `ENE_STAGE_POC_SECONDS` auto-exits after N seconds
(used for idle / animation samples).

---

## Experiment A — shared wgpu + Slint composition

### What was implemented

The probe **owns** the winit window and the wgpu surface, matching
`ene-stage`'s overlay (`apps/ene-stage/src/overlay.rs`):

1. Create a `GpuContext` with the same backend rules as stage
   (PRIMARY / Vulkan on Linux, DX12 + `DxgiFromVisual` on Windows).
2. Create one transparent, decoration-less, always-on-top window.
3. `slint::platform::set_platform` with a custom `WindowAdapter` whose
   renderer is `FemtoVGWGPURenderer::new(instance, device, queue)` —
   clones of the **same** wgpu objects.
4. Each frame, GPU-only:

   wgpu 3D (triangle or `ene-vrm`) → clear transparent
   → Slint FemtoVG into an offscreen `Rgba8Unorm` texture
   → GPU blit (premultiplied) onto the surface
   → `present`

Slint does **not** own a second swapchain. `FemtoVGWGPURenderer::render_to_texture_view`
draws into a texture we allocate on the shared device.

VRM: `ene-vrm` loads `assets/characters/Alicia/AliciaSolid.vrm` when
that file is a real GLB (>1 KiB). Otherwise it writes
`ene_vrm::minimal` to a temp path. The VRM renderer is the same
`VrmRenderer::render` used by the overlay (command encoder + depth,
no CPU readback). If VRM load is skipped, the colored triangle is
enough to prove the pass order; VRM plugs into the same
`encoder` + surface view.

### Shared Device/Queue

**Yes, by construction.** `GpuHandles` clones `Instance` / `Device` /
`Queue` from `GpuContext` and passes them to
`FemtoVGWGPURenderer::new`. wgpu 29 `Device`/`Queue` are cheap clones
of the same GPU objects. The triangle / VRM pipelines are created on
`GpuContext.device`; Slint's FemtoVG uses the clone.

The official `BackendSelector::require_wgpu_29(WGPUConfiguration::Manual)`
path was **not** used for the main probe: that lets Slint own the
window/surface. The question was whether Slint requires its own
surface — it does not, if you use the custom-platform + FemtoVG WGPU
renderer.

`unstable-wgpu-29` is an unstable feature. A wgpu 30 bump in Slint
will require `unstable-wgpu-30` and a crate bump. That is a
maintenance cost, not a runtime blocker.

### Zero-copy composition

**Yes for the 3D + UI path.** There is no `copy_texture_to_buffer`,
`map_async`, or `SharedPixelBuffer` on the composition path (enforced
by a unit test that scans the source). Slint writes a GPU texture;
we blit it. The extra blit is a GPU copy of the UI layer, not a
GPU→CPU→GPU round trip.

A second compositor pass (3D then UI blit) is required because
FemtoVG's WGPU renderer clears its target. Drawing Slint directly
onto the surface would wipe the 3D. The offscreen UI texture + blit
preserves **3D → UI → present**.

### Transparent window

Uses the same `pick_alpha_mode` / `CompositeAlphaMode::{Pre,Post}Multiplied`
logic as `ene-stage`. Linux with lavapipe typically offers
`PreMultiplied`. Windows still needs `with_no_redirection_bitmap(true)`
+ DX12 visual swapchain (copied from `gpu.rs`).

### Performance

Filled from a Cloud Agent Linux run (`DISPLAY=:1`, lavapipe, debug
build) after `ENE_STAGE_POC_SECONDS=8`.

Adapter: `llvmpipe (LLVM 20.1.2, 256 bits)`, backend Vulkan.
`transparency=true` (`Bgra8UnormSrgb`, `PreMultiplied`).
VRM: `ene_vrm::minimal` fixture (`assets/characters/Alicia/*.vrm`
is not in this workspace). VRAM is **not** exposed by lavapipe;
RSS is the proxy. First-frame / shader-compile hitches dominate
`max_ms`.

| Probe | Phase | frames | avg frame | max frame | CPU user | CPU sys | RSS start | RSS end |
|---|---|---|---|---|---|---|---|---|
| A Slint+VRM | idle 3s | 2 | 135.1 ms | 169.5 ms | 280 ms | 20 ms | 122 MiB | 167 MiB |
| A Slint+VRM | animation 5s | 657 | 11.77 ms | 2740 ms | 10480 ms | 610 ms | 167 MiB | 169 MiB |
| B input | idle 8s | 2 | 133.6 ms | 162.4 ms | 260 ms | 50 ms | 122 MiB | 167 MiB |
| egui+triangle | idle 3s | 1 | 21.1 ms | 21.1 ms | 20 ms | 0 | 114 MiB | 137 MiB |
| egui+triangle | animation 5s | 2958 | 2.70 ms | 2990 ms | 6020 ms | 1570 ms | 137 MiB | 141 MiB |

Idle is real `WaitUntil` (a couple of presents, then sleep). Animation
is `Poll`. Slint+VRM is slower than the egui triangle baseline because
the probe draws a VRM, an offscreen UI target, and a blit — not
because FemtoVG is inherently 4× slower. On lavapipe, animation CPU
exceeds wall time (software raster). This is not a product-GPU
number.

Shared Device/Queue: **yes**. Log line
`FemtoVGWGPURenderer constructed from cloned GpuContext instance/device/queue`.
GPU→CPU copies: **none** on the composition path (source scan + no
`map_async` in frame code).

### Problems

- `FemtoVGWGPURenderer` + custom platform is **unstable API**.
- FemtoVG clears its target, so UI cannot share the 3D render pass;
  a blit (or Slint `Image` underlay) is required.
- Slint's `set_rendering_notifier` underlay on FemtoVG has a known
  per-frame pipeline-recreation bug. This probe avoids that API.
- `slint::slint!` `Button` needs `std-widgets.slint`. Hex colors like
  `#8ecbff88` parse as scientific notation (`8e…`); use digits without
  `e` after `#`.
- wgpu 29 pipeline descriptors in this tree use `multiview_mask`,
  `depth_write_enabled: Some(…)`, and `bind_group_layouts: &[Some(…)]`.
  Slint examples on older wgpu will not copy-paste.

### Slint adoption blocker?

**No runtime blocker for composition** if the custom platform is
accepted. The maintenance risk of `unstable-wgpu-29` is real but
expected: ene already pins wgpu 29.

---

## Experiment B — input routing / VRM hit-test / passthrough

### Routing

Explicit pure functions in `crates/ene-stage-poc/src/input.rs`:

```
Pointer → UI rects → VRM coarse parts → passthrough
```

UI wins overlaps. Clicks print:

```
UI hit: …  VRM hit: …  passthrough: …  target: …
```

Unit tests cover priority, hand-over-torso, empty passthrough, and
OS input-region union without opening a window.

### VRM hit-test

CPU coarse screen-space rects: **head, torso, left hand, right hand**.
If a VRM is loaded, the layout is derived from the normalized AABB
(same auto-fit as the overlay). Otherwise a placeholder covering the
triangle is used.

**GPU picking was not implemented.** Reasons:

- The required model is "which body part / empty desktop", not
  per-triangle selection.
- GPU picking needs an ID buffer and a readback. Even a small cursor
  tile is a GPU→CPU sync; doing it every frame, or while the cursor is
  still, is the cost model this probe forbids.
- Stage already ships AABB raycasts (`ene-stage` `drag::hit_test`)
  that are good enough for silhouette interaction.
- A later optional path: CPU coarse miss → skip; CPU hit on overlapping
  parts → optional small-tile GPU pick. Not needed to prove the input
  model.

### OS-level click-through

`StageInputRegion` (`set_passthrough`, `update_input_region`).

This is **not** the same as in-process routing. The compositor only
delivers events inside the OS input region. If the region is empty,
ene never sees the click — that is the desired passthrough.

| OS | Whole-window click-through | Interactive subset | Notes |
|---|---|---|---|
| **Windows** | Yes. `set_cursor_hittest(false)` + `WS_EX_TRANSPARENT`. | Yes, via `SetWindowRgn` union of UI+VRM rects (not per-pixel alpha). `WM_NCHITTEST` / DComp hit-testing would be a later refinement. | Matches stage overlay's HWND path. |
| **X11** | Yes. XShape `Kind::Input` empty list. Protocol **works**: SHAPE 1.1 is present, `shape::rectangles(SET, INPUT)` succeeds, and same-connection `get_rectangles` returns the UI∪VRM set. | Yes, rectangle union. | **Compositor caveat (measured on xfwm4):** the WM reparents the client and, within about a frame, restores a full-window Input region. External `XShapeGetRectangles` then shows `0,0 800×600`, and empty-glass clicks still reach the client. `_NET_WM_BYPASS_COMPOSITOR=1` did not stop that reset here. In-process routing still classifies those clicks as `Passthrough`. Stage today mostly toggles `set_cursor_hittest` (no-op on Linux). Desktop already uses shape. **X11 can query the global pointer** (`query_pointer`) so a fully click-through window can re-arm when the cursor returns. Need per-WM verification (openbox / no compositor / picom). |
| **Wayland** | Yes. `wl_surface::set_input_region` with an empty region. | Yes, rectangle union. **Not** per-pixel. | **No global pointer query.** If the region is empty, the client receives **no** pointer events and cannot hover-open a hole. The viable design is: OS region = UI ∪ VRM coarse rects at all times. Layer-shell (`zwlr_layer_shell_v1`) is optional for stacking above fullscreen; it is not required for input regions. `winit 0.30` `set_cursor_hittest` is a **no-op on Linux** (documented in `ene-desktop` and `ene-stage` platform modules). |

**Wayland is not a blocker for the input model**, but it **is** a
blocker for "full-window click-through + hover to punch a hole".
ene-stage already documents that Wayland users must turn click-through
off to drag. The PoC's `update_input_region(rects)` is the design that
closes that gap.

This Cloud Agent session runs under `DISPLAY=:1` (X11, xfwm4). Wayland
code is compiled and the protocol usage matches `ene-desktop`'s
`wayland_region.rs`. It was not compositor-tested here.

### Click log (Experiment B, real pointer)

Window-relative clicks via `xdotool mousemove` then `click` (not
`click --window`, which would XSendEvent and skip hit-testing):

```
UI hit: true   VRM hit: false  passthrough: false  target: Ui
UI hit: false  VRM hit: true   passthrough: false  target: Vrm(Torso)
UI hit: true   VRM hit: false  passthrough: false  target: Ui   # overlap
UI hit: false  VRM hit: false  passthrough: true   target: Passthrough
```

Empty-glass events still arrived at the client on this xfwm4 session
(see compositor caveat above). The in-process router did **not**
treat them as UI or VRM.

### Performance concerns

Routing is a handful of AABB tests per pointer event. Updating the OS
region on layout/animation should be throttled in production (desktop
already does this on `about_to_wait`). No GPU readback.

### Blocker?

**No.** Platform differences must be encoded in `StageInputRegion`,
not assumed away.

---

## Measurements (Cloud Agent Linux)

See Experiment A performance table. Raw printer lines:

```
=== experiment-a ===
adapter: llvmpipe (LLVM 20.1.2, 256 bits)
backend: Vulkan
shared_device=true transparency=true vrm=true input=x11 partial_region=true zero_copy=gpu-texture-blit
phase=idle wall_ms=3000.5 frames=2 avg_ms=135.10 max_ms=169.51 cpu_user_ms=280.0 cpu_sys_ms=20.0 rss_start_kib=125168 rss_end_kib=170852
phase=animation wall_ms=5005.6 frames=657 avg_ms=11.77 max_ms=2740.44 cpu_user_ms=10480.0 cpu_sys_ms=610.0 rss_start_kib=170852 rss_end_kib=173092

=== experiment-b ===
phase=idle wall_ms=8000.7 frames=2 avg_ms=133.63 max_ms=162.39 cpu_user_ms=260.0 cpu_sys_ms=50.0 rss_start_kib=124828 rss_end_kib=170832

=== egui-baseline ===
phase=idle wall_ms=3000.3 frames=1 avg_ms=21.13 max_ms=21.13 cpu_user_ms=20.0 cpu_sys_ms=0.0 rss_start_kib=116288 rss_end_kib=140176
phase=animation wall_ms=5000.9 frames=2958 avg_ms=2.70 max_ms=2989.63 cpu_user_ms=6020.0 cpu_sys_ms=1570.0 rss_start_kib=140176 rss_end_kib=144556
```

VRAM: unavailable on lavapipe (software Vulkan). No `nvidia-smi`.

GPU→CPU copies: none on the composition path (source scan + no
`map_async` in frame code).

---

## Manual verification (not unit-tested)

OS compositor behavior is not unit-tested. On a real desktop:

1. Run `ene-stage-poc-a`. Confirm a transparent window, 3D (or
   triangle) behind a rounded bubble, button click, resize, and DPI
   (move across monitors).
2. Run `ene-stage-poc-b`. Click the bubble → `target: Ui`. Click the
   character → `target: Vrm(…)`. Click empty glass → the window does
   not consume the click (terminal / window below activates) and the
   log shows `passthrough: true` only if the event still reached the
   client (X11 full-window hit-test). With a correct input region,
   empty clicks never arrive.
3. Overlap the bubble over the body → UI wins.
4. Windows: confirm `WS_EX_TRANSPARENT` when the region is empty.
5. Wayland: confirm `wl_surface.set_input_region` with Sway/KWin;
   do not expect hover-through an empty region.

---

## Final verdict

**B. Some architecture changes are required, but this design can
proceed.**

Not **A**, because:

- Slint cannot draw into the 3D pass without a blit or an `Image`
  underlay (FemtoVG clears).
- Wayland forbids "click-through everything, then hover to interact"
  without an explicit input region.
- `unstable-wgpu-29` and the custom `Platform` are a maintenance
  surface.

Not **C**, because:

- Shared Device/Queue is a supported Slint API (`FemtoVGWGPURenderer`
  and `WGPUConfiguration::Manual`).
- Zero-copy GPU composition is real.
- Transparency matches the existing overlay path.
- Input routing is a pure function plus a small OS abstraction that
  ene-desktop already proved on X11/Wayland.

Criteria:

| Concern | Result |
|---|---|
| Performance | Extra UI blit is cheap on a real GPU; lavapipe animation is CPU-bound as expected. No readback. |
| Memory | Idle RSS ~167 MiB Slint+VRM vs ~137 MiB egui+triangle. One extra window-sized Rgba8 UI target (~2 MiB). |
| Input correctness | Pure-function tests pass. On this xfwm4 session, empty-glass still reached the client; the router labeled `Passthrough`. Wayland input regions remain the portable OS model. |
| Transparency | Same alpha-mode picker as stage. |
| Platform portability | Windows / X11 / Wayland all possible; APIs differ. |
| Maintenance | Unstable Slint wgpu feature; pin and bump with wgpu. |

---

## What this probe did not do

- No Slint port of chat / detail / theme.
- No deletion of egui.
- No change to the production overlay.
- No GPU picking.
- No Wayland compositor run in this Cloud Agent VM (`DISPLAY=:1` is X11).
