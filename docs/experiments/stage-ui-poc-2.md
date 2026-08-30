# Stage UI probe 2: fair compositor cost and Linux input regions

Follow-up to [Stage UI probe](stage-ui-poc.md). Still **not a product
path.** Production `ene-stage` is unchanged.

Code: `crates/ene-stage-poc/`

| Binary | Experiment |
|---|---|
| `ene-stage-poc-c` | C: fair compositor cost (C0–C4) |
| `ene-stage-poc-d` | D: Linux OS input region / click-through |
| `ene-stage-poc-click-sink` | Separate process that records real pointer clicks |

Windows is **out of scope** for this follow-up. Linux only: X11 and
Wayland are measured as separate backends.

```sh
# Experiment C (release; default 5 s warmup + 12 s measure + 5 s idle)
cargo build --release -p ene-stage-poc --bins
DISPLAY=:1 WGPU_BACKEND=vulkan ./target/release/ene-stage-poc-c

# Experiment D
DISPLAY=:1 WGPU_BACKEND=vulkan ENE_STAGE_POC_SECONDS=12 ./target/release/ene-stage-poc-d
ENE_STAGE_POC_SHAPE_KIND=both   # X11: Bounding+Input (required on xfwm4)
ENE_STAGE_POC_X11_TARGET=client|frame|both
ENE_STAGE_POC_OVERRIDE_REDIRECT=1
ENE_STAGE_POC_MOVE_VRM=1
ENE_STAGE_POC_REGION_PX=2 ENE_STAGE_POC_REGION_MS=16
```

---

## Environment

This Cloud Agent VM:

| Item | Value |
|---|---|
| OS | Linux 6.12, Xorg 21.1.11 |
| Desktop | XFCE |
| X11 WM | **xfwm4** (compositing **on**) |
| Display | `DISPLAY=:1` |
| GPU | **no `/dev/dri`** — software Vulkan only |
| Adapter | `llvmpipe (LLVM 20.1.2, 256 bits)` |
| Driver | `llvmpipe` / Mesa 25.2.8 (LLVM 20.1.2) |
| wgpu backend | Vulkan |
| Build | `cargo build --release -p ene-stage-poc` |
| VRM | `ene_vrm::minimal` fixture (Alicia is not in the workspace) |
| Window | 800×600, transparent, undecorated unless noted |

A real GPU was not available. Debug+lavapipe numbers from probe 1 are
**not** used for the C verdict. The C numbers below are **release +
lavapipe**. They are a fair *delta* comparison (same window, same VRM,
same wgpu, same resolution, same loop). Absolute FPS will be higher on a
hardware adapter; the ranking of C0–C4 should still hold.

GPU hardware frame time and VRAM counters are unavailable: the adapter
does not expose `nvidia-smi` / DRM memory, and the probe does not enable
wgpu timestamp queries. RSS is the memory proxy.

---

## Compositor (what the PoC actually does)

Do not call this "zero-copy". Split:

| Property | C0 | C1–C3 (Slint) | C4 (egui) |
|---|---|---|---|
| CPU readback | no | no | no |
| GPU resident | yes | yes | yes |
| `copy_texture_to_texture` | no | no | no |
| GPU render-pass composition | no | **yes** | yes (Load onto swapchain, no offscreen UI target) |

C1–C3:

```
VRM  → swapchain (clear + 3D pass)
Slint FemtoVG → offscreen Rgba8Unorm target (this pass clears its target)
fullscreen triangle, sample UI texture
  blend = wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING
           (src=One, dst=OneMinusSrcAlpha)
  load  = LoadOp::Load  (keep VRM)
→ present
```

That is a **fullscreen alpha composite pass**, not a blit via
`copy_texture_to_texture`. Bind groups are rebuilt each frame (cheap).
FemtoVG cannot share the VRM pass because it clears its color target.

Premultiplied alpha: the composite pass uses premul blend. FemtoVG's
WGPU renderer writes premultiplied RGBA. Empty C1 (bubble hidden) leaves
the VRM looking like C0; that matches premul (0,0,0,0) over VRM.

C4 draws an equivalent bubble with egui `LoadOp::Load` directly on the
swapchain (same style as production chrome), **not** through the Slint
offscreen target. That is the toolkit difference being measured.

Source scan in `ene-stage-poc` tests forbids `map_async`,
`copy_texture_to_buffer`, `copy_texture_to_texture`, and
`SharedPixelBuffer` on the composition path.

---

## Experiment C — fair compositor cost

Same VRM, 800×600, Vulkan llvmpipe, release, `AutoVsync`. Warmup 5 s
(shader / pipeline hitch), then ≥10 s measure, then 5 s idle with
`ControlFlow::WaitUntil` and **no** redraw.

Idle after the first C run was a bug: `rotate_phase` cleared the frame
list, which kept `Poll` spinning. Fixed; idle is now 0 frames and 0
process CPU.

### Cases

| Case | Path |
|---|---|
| C0 | VRM → surface. No UI renderer. Baseline. |
| C1 | C0 + Slint init + empty offscreen target + composite pass. Bubble hidden. |
| C2 | C1 + static bubble (translucent rounded rect, text, button). |
| C3 | C2 + light animation (opacity, Y position, scale, blinking cursor). |
| C4 | C0 + egui bubble equivalent, Load pass on the swapchain. |

### Raw + warmup-excluded (measure)

`startup_ms` is process start → first present (includes hitch).
**Hitch** is warmup `max_ms`. **Steady-state** is the measure phase.

From `ene-stage-poc-c` release on this VM:

| Case | startup_ms | warmup max_ms (hitch) | measure fps | avg_ms | median | p95 | p99 | max_ms | CPU user_ms / 12 s | RSS end |
|---|---|---|---|---|---|---|---|---|
| C0 | 94 | 15.4 | 507 | 1.97 | 1.91 | 2.45 | 2.90 | 14.0 | 27780 | 122 MiB |
| C1 | 137 | 39.7 | 210 | 4.76 | 4.66 | 5.55 | 6.06 | 6.95 | 34890 | 130 MiB |
| C2 | 239 | 124 | 184 | 5.44 | 5.39 | 6.19 | 6.65 | 11.2 | 33170 | 149 MiB |
| C3 | 199 | 119 | 182 | 5.49 | 5.76 | 6.98 | 7.41 | 13.4 | 30490 | 155 MiB |
| C4 | 81 | 12.5 | 482 | 2.08 | 2.02 | 2.53 | 2.82 | 4.45 | 27130 | 127 MiB |

Idle (all cases): **0 frames, 0 CPU, RSS unchanged**. Event loop sleeps.

Animation CPU is the measure-phase user CPU (continuous redraw). Idle
CPU is the idle phase (must stay ~0).

### Deltas (measure avg frame)

| Delta | avg_ms | What it is |
|---|---|---|
| **C1 − C0** | **+2.79 ms** (1.97 → 4.76) | Slint infrastructure + offscreen target + composite pass |
| **C2 − C1** | **+0.68 ms** | Static bubble (text / rounded rect / button) |
| **C3 − C2** | **+0.05 ms** | Light animation |
| **C2 vs C4** | 5.44 vs 2.08 (**2.6×** on lavapipe) | Slint offscreen+blit vs egui Load-on-swapchain |

RSS: C1−C0 ≈ +8 MiB (Slint + one window-sized RGBA target). C2−C1 ≈
+19 MiB (fonts / widgets). C4 stays close to C0 (+5 MiB).

### Slint performance verdict

**許容可能 (acceptable).** Not a blocker.

The cost that matters is C1−C0 (compositor / FemtoVG init), not the
widgets and not the animation. On software Vulkan that is a real
+2.8 ms and about 2.4× frame time vs VRM-only, but still ~210 FPS with
huge 60 FPS headroom. C2 vs C4 is not "Slint is 2.6× slower at drawing
a bubble"; C4 skips the offscreen target. C1 is already most of the
gap.

A hardware GPU should shrink C1−C0. It cannot be claimed from this VM.
If a future product target is software raster at 4K, that would be
**要最適化**. For Stage on a real GPU, this is not a reason to reject
Slint.

---

## Experiment D — Linux input region / click-through

OS region = **UI interactive bounds ∪ VRM coarse bounds** (head,
torso, left hand, right hand). Generated by a pure function
`build_input_regions`. Updates only when a rect moves/resizes by more
than `ENE_STAGE_POC_REGION_PX` (default 2 px) and at most every
`ENE_STAGE_POC_REGION_MS` (default 16 ms).

**Not** "make the whole window click-through and re-arm on hover."

Process-local routing: UI > VRM > none. Overlap is always UI. Logs:

```
UI hit: …  VRM hit: …  OS region hit: …  target: …
```

Unit tests cover union, priority, overlap, background, multiple UI /
VRM rects, moving threshold, AABB, rate limit, hidden UI, empty
region.

Success for OS click-through is **another process receiving the click**,
not an in-process `Passthrough` label.

### X11 (xfwm4 / XFCE)

SHAPE 1.1, XFixes 5.0. Client is reparented (`XQueryTree`: client →
frame → root). `_NET_WM_BYPASS_COMPOSITOR=1` is set. It does **not**
stop the reset.

| Variant | After SET | ~200 ms later | Empty-glass → click-sink process? | Empty-glass → poc-d? |
|---|---|---|---|---|
| Input on **client** | Input = UI∪VRM | client+frame Input **reset to 800×600** | **no** | yes, `target: Passthrough` |
| Input on **frame** | frame Input = UI∪VRM; client already full | both full | no | yes, Passthrough |
| Input on **both** | both set | both reset to full | no | yes, Passthrough |
| `override_redirect` | no frame (parent=root) | **still** Input reset to full | no | yes, Passthrough |
| decorated | frame Bounding is the decorated silhouette | Input reset on client | no | (title bar offsets clicks) |
| **Input + Bounding (client)** | both = UI∪VRM | Input reset to full; **Bounding stays UI∪VRM** | **yes (`SINK CLICK`)** | **no event** (UI/VRM clicks still arrive) |

**Cause of the Experiment B reset:** xfwm4's compositor overwrites
**Shape Input** on ARGB windows within about one frame, including
override-redirect clients. It does **not** restore Bounding to the
full window. Hit-testing uses Bounding ∩ Input, so a Bounding hole is
enough for clicks to miss the client even after Input is full again.

Input-only Shape is therefore **not** a working click-through API on
this WM. **Bounding ∪ Input** is.

Constraints: Bounding punches real holes (coarse rectangles, not a
pixel-perfect VRM silhouette). Rounded bubble corners become rects.
Must not update Shape every frame (see cost below).

### Wayland (Weston 13)

Nested compositor: `weston 13.0.0`, **pixman** renderer, X11 backend
window 1280×800, socket `ene-poc`. Clients launched with
`WAYLAND_DISPLAY=ene-poc` and **`DISPLAY` unset** so winit is a **native
Wayland** client, not XWayland.

Log:

```
native_wayland=true
Wayland wl_surface input region attached
server=wayland
```

`wl_surface::set_input_region` is called with the UI∪VRM rects (or empty
for empty scenes).

| Check | On this Weston |
|---|---|
| Native Wayland (not XWayland) | yes |
| In-region pointer events | yes: `target: Ui`, `Vrm(Head)`, `Vrm(LeftHand)` while moving |
| Empty region → no events on poc-d | yes (no `Passthrough` line for empty-glass) |
| Empty region → other client | weston **desktop-shell background** received the click; a winit click-sink stacked under the overlay did not always sit under the hole. Not claimed as a second-process log on this compositor. |
| Moving region | 8.4 OS updates/s with default throttle; hits still classified |

**This is success on Weston 13, not "Wayland in general."** Mutter / KWin /
wlroots / Hyprland were not run.

Wayland OS apply is cheap here (~12 µs vs hundreds of µs for X11 Shape).

### Region update cost

| Run | OS updates | apply_hz | gen_avg | apply_avg |
|---|---|---|---|---|
| Static (6 s) | 1 | 0.17 /s | ~0 µs | 3.3 ms first SET |
| Moving, 2 px / 16 ms | 50 | **8.3 /s** | ~0–1 µs | 0.7 ms |
| Moving, no throttle | 2161 | **360 /s** | 1 µs | 0.37 ms |
| Moving, 4 px / 50 ms | 26 | 4.3 /s | ~0 µs | 0.9 ms |
| Wayland moving (20 s) | 168 | 8.4 /s | 1 µs | **12 µs** |

**60 OS updates per second are not required.** Generation is a pure
AABB union (nanoseconds). The X11 SET is the expensive part. Default
dirty threshold + rate limit keeps moving VRM around 8 Hz. Recommended
production policy: dirty flag + pixel threshold + 30–50 ms rate limit.

### Fallback (evaluation only)

| Layer | Proposal |
|---|---|
| Wayland | Dynamic `set_input_region` from scene geometry (works on Weston 13). |
| X11 when Shape Input is stable | Input-only union. **Not** xfwm4. |
| X11 on xfwm4 / similar compositors | Set **Bounding + Input** to the coarse union (measured working). |
| Coarser X11 | Larger Bounding rects if SET rate or visual holes bother the user. |
| Last resort | User setting: window-wide click-through. Not needed to ship the Linux design. |

### Platform abstraction

Keep `StageInputRegion` (`update_input_region(&[Rect])`):

- Linux Wayland → `wl_surface::set_input_region`
- Linux X11 → Shape **Bounding + Input** on the client (and frame if a
  given WM needs it)
- Pure `build_input_regions` / `classify_pointer` stay in-process and
  OS-agnostic

Windows is deferred.

---

## Final verdict

**B. The basic design holds; Linux needs a platform-specific X11
fallback (Bounding shape on compositing WMs such as xfwm4).**

Not **A**: Input-only Shape does not click through on xfwm4; product
code must treat X11 Bounding vs Wayland `set_input_region` as different
backends.

Not **C**: compositor cost is acceptable; idle is real idle; Wayland
input regions work on Weston; X11 click-through **does** work once
Bounding is set; in-process UI > VRM > none is unit-tested.

| Concern | Result |
|---|---|
| Performance | C1−C0 is the Slint cost; still ≫ 60 FPS on lavapipe. No CPU readback. |
| Memory | +8 MiB compositor, +19 MiB static widgets vs C0. Fine. |
| Compositing | Fullscreen premul pass; no texture copy; no readback. |
| Click-through | Proven on xfwm4 with Bounding+Input + click-sink process. |
| X11 | Input-only is reset; Bounding sticks. Requires the fallback. |
| Wayland | Works on **Weston 13** as a native client. Not generalized. |
| Maintenance | Two Linux backends behind one rect-union API. Fine. |

### What this probe did not do

- No Slint port of chat / detail / theme.
- No production overlay change.
- No hardware GPU numbers.
- No Mutter / KWin / Sway / Hyprland run.
- No Windows evaluation.
