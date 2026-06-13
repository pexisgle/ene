# `ene-desktop`: Bevy → wgpu / winit / egui Migration Plan

> **Status:** Approved design — partially implemented.
> **Scope:** `apps/ene-desktop-v2` (new, winit + wgpu), `crates/ene-vrm` (new, stub) and the legacy `apps/ene-desktop` (Bevy, still in place). Everything else (`ene-core`, `ene-memory`, `ene-cli`, tool binaries) is untouched.
> **Owner / Driver:** TBD
> **Target outcome:** Resolve two long-standing correctness issues on the desktop app and produce a slimmer, more maintainable rendering stack we fully own.

---

## 0. Current Implementation State

This document is the **design plan** for the migration. The tables below summarise what is actually on disk at the time of writing, so a reader does not have to diff the workspace to know which phases have landed.

| Phase | Plan ref | State | Notes |
|-------|----------|-------|-------|
| **PR0 — `ene-desktop-v2` scaffold + Windows transparency smoke** | §22.3 | **Shipped** | `apps/ene-desktop-v2/` (~330 lines across `main.rs`, `gpu.rs`, `runtime.rs`) replaces the planned 7-module split. Single transparent window, red-quad renderer, `Space` toggles transparency, `Escape` exits. Verified on the developer's Windows machine. |
| **PR1 step 1 — `ene-vrm` crate skeleton** | §4 PR1 / step 2 | **Shipped** | `crates/ene-vrm/{Cargo.toml, src/lib.rs}` created with an empty `pub fn version()` stub and a unit test. No `gltf` / `wgpu` / `winit` deps yet. |
| **PR1 steps 3–8 — strip Bevy from `apps/ene-desktop`, wire winit window + tray + AI bridge** | §4 PR1 / steps 3–8 | **Not started** | The legacy `apps/ene-desktop` is still Bevy 0.18, unchanged. The `patches/bevy_winit/` patch has been removed (the legacy build now uses upstream `bevy_winit`); `apps/tw-test/` has also been removed. |
| **PR2 — egui integration + settings window** | §4 PR2 | **Not started** | — |
| **PR3 — `ene-vrm` static rendering (MToon + skinning)** | §4 PR3 | **Not started** | — |
| **PR4 — Expressions, LookAt, BodyTracking** | §4 PR4 | **Not started** | — |
| **PR5+ — VRMA, spring-bone, FXAA/SMAA, drag polish** | §4 PR5+ | **Not started** | — |

### 0.1 How the two desktop apps coexist

Until PR1's "skeleton swap" step 3 finishes, both binaries build side-by-side:

- **`apps/ene-desktop`** — legacy Bevy 0.18 build. Still the user-facing desktop app. Still depends on `bevy`, `bevy_egui`, `bevy_vrm1`, `tray-icon`, `gtk` (Linux), and `wayland-client` (Linux). The local `patches/bevy_winit` patch has been removed; the legacy build now uses upstream `bevy_winit` 0.18 from crates.io. The legacy binary is **not** being modified by this migration until PR1 step 3.
- **`apps/ene-desktop-v2`** — new crate, lives next to the legacy one. Renders a single transparent window with `winit` + `wgpu` 27 and a hard-coded red quad. **Cargo `run -p ene-desktop-v2`** to launch.

Once PR1 is finished, `apps/ene-desktop` will be deleted and the `apps/ene-desktop-v2` sources will be moved to `apps/ene-desktop`. The migration plan (§4) calls this out as "PR1 is the deletion step." Until then, treat them as parallel codebases.

### 0.2 Where the recipe was proven

The transparency recipe that PR0 ships was originally proved out in `apps/tw-test`, a standalone Bevy 0.18 testbed. The cross-reference to that testbed is preserved in §22.3 below for historical context; the testbed itself has been deleted along with the `patches/bevy_winit` patch as they are no longer needed.

---

## 1. Background and Motivation

The current `ene-desktop` implementation is built on top of Bevy 0.18 (`bevy_winit`, `bevy_egui`, `bevy_vrm1`, …). Two specific bugs make the production build unusable on Windows:

| # | Bug | Trigger | Effect |
|---|-----|---------|--------|
| B1 | egui rendering crash | Windows + DX12 + `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` | egui panics or crashes during rendering. We currently work around it by leaving the env var unset and accepting that window transparency is not pixel-perfect. |
| B2 | Window transparency broken | Windows + Vulkan backend | The character window is fully opaque even with `transparent: true`. The whole desktop overlay concept is unusable. |

The two bugs together motivated carrying a local patch of `bevy_winit` (deleted with PR0 / step 1 of this migration) to work around winit issues inside the Bevy wrapper. With the patch gone, the legacy Bevy build uses upstream `bevy_winit` 0.18; the v2 stack no longer has this technical debt.

### 1.1 Why drop Bevy (not patch it)

- The two bugs sit **at the seam between Bevy's `Window`s and wgpu surfaces**, not inside our code. We cannot fix them without forking Bevy itself.
- We only use a thin Bevy slice: a window, a `Camera3d`, a render layer split, an asset server, the egui bridge, and `bevy_vrm1` for the model. Most other features (system tray, hotkeys, settings persistence, AI bridge) are already hand-rolled outside Bevy.
- `bevy_vrm1` itself uses the upstream `gltf` crate and applies the MToon material in WGSL. We can reuse that shader logic and the glTF walking code, just stripped of the `bevy_pbr` dependency.
- A direct stack is ~5–10× less binary, has zero indirect dependencies through `bevy_ecs` macros, and we stop maintaining a fork of `bevy_winit`.

### 1.2 Why a separate `ene-vrm` crate

- The VRM code (gltf walking, MToon shader, expression/lookat, future spring-bone) is non-trivial and has its own tests. Keeping it inside `ene-desktop` would re-tangle rendering and app wiring.
- A standalone crate gives us a clean unit to test, document, and (later) re-use in `ene-cli` for headless rendering or screenshot tests.
- It also lets us switch shader backends (wgpu 27) without touching the desktop binary.

---

## 2. Goals and Non-Goals

### 2.1 Goals (must ship)

- G1. Windows 10/11 (DX12): `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` set, egui and 3D composite correctly into a transparent window.
- G2. Linux + X11 (Vulkan): character window is transparent; settings window is opaque.
- G3. Linux + Wayland (Vulkan, layer-shell where available): character window is transparent and uses a proper layer to keep compositor blur/clipping working.
- G4. `crates/ene-vrm` loads a glTF / VRM 1.0 file, computes skin matrices, draws with MToon, and exposes a `LookAt` / expression API.
- G5. System tray, hotkeys, settings persistence, AI bridge (ene-core `EneHandle`) keep working unchanged from the user's point of view.
- G6. Existing public configuration schema (`assets/character_settings.schema.json` etc.) is unchanged.

### 2.2 Non-Goals (explicitly deferred)

- N1. VRMA playback and spring-bone simulation. PR5+; tracked separately.
- N2. Per-frame shadow quality switching (FXAA / SMAA / TAA toggles). We keep a single default for now.
- N3. A new public C-ABI / plugin surface for the VRM crate.
- N4. macOS support. The new code compiles on macOS using Vulkan but is not provisioned for daily testing.
- N5. A unified cross-platform "transparency abstraction" library — we accept platform-specific paths.

### 2.3 Out-of-Scope Cleanup (do opportunistically)

- Removing the now-obsolete `bevy_winit` patch in `patches/`.
- Removing `bevy`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1` from the workspace dependency graph.

---

## 3. High-Level Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                       apps/ene-desktop                              │
│  ┌──────────────────┐  ┌─────────────────────┐  ┌──────────────┐  │
│  │ main.rs          │  │ runtime/            │  │ tray.rs      │  │
│  │ (event loop,     │  │  event loop pump    │  │ system tray  │  │
│  │  startup)        │  │  WindowSlot manager │  └──────────────┘  │
│  └────────┬─────────┘  └──────────┬──────────┘                     │
│           │                       │                                 │
│  ┌────────▼─────────┐  ┌──────────▼──────────┐                     │
│  │ gpu/             │  │ ui/                 │                     │
│  │  wgpu device,    │  │  egui contexts,     │                     │
│  │  depth, camera   │  │  settings windows   │                     │
│  └────────┬─────────┘  └──────────┬──────────┘                     │
│           │                       │                                 │
│  ┌────────▼─────────┐  ┌──────────▼──────────┐  ┌──────────────┐   │
│  │ character/       │  │ ai_bridge.rs        │  │ platform/    │   │
│  │  drag, cursor,   │  │  EneHandle poll     │  │  HWND,       │   │
│  │  VrmHandle glue  │  │  VecDeque<EneEvent> │  │  wayland lyr │   │
│  └────────┬─────────┘  └─────────────────────┘  └──────────────┘   │
│           │                                                          │
└───────────┼──────────────────────────────────────────────────────────┘
            │ uses
            ▼
┌──────────────────────────────────────────────────────────────────────┐
│                       crates/ene-vrm                                │
│  loader (gltf) → model (VrmModel) → skeleton → mtoon (WGSL)        │
│  → renderer (draws into a wgpu::Surface)                            │
│  expression, look_at, (later) spring_bone, vrma                     │
└──────────────────────────────────────────────────────────────────────┘
            │ uses
            ▼
     wgpu 27, winit 0.30, egui 0.33 (egui-wgpu, egui-winit)
     gltf 1.4, glam 0.29, encase 0.12, bytemuck, pollster
```

### 3.1 Module boundaries

- `runtime/` owns the `winit::EventLoop`, the `wgpu::Instance / Device / Queue`, and a `HashMap<WindowId, WindowSlot>`. Nothing in this layer knows about VRM models or settings.
- `gpu/` is a small, generic helper for creating a wgpu device with the right features and a depth buffer. No app-specific state.
- `ui/` is a per-window egui integration. Each `WindowSlot` owns its `egui::Context`, `egui_winit::State`, and `egui_wgpu::Renderer`.
- `character/` consumes `ene_vrm::VrmRenderer` and binds it to a `WindowSlot`.
- `ene-vrm` is platform-agnostic. It takes a `wgpu::Device`, a `wgpu::RenderPass`, and a `CameraUniform` and renders. It does not know about winit.

---

## 4. Phased PR Plan

We deliver the migration as a series of small, individually reviewable PRs. Each PR must keep `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` green. **Direct commits to `main` remain acceptable during the early-development phase per AGENTS.md §10.**

### PR1 — Skeleton swap (the hardest PR)

> **Status:** In progress. **Step 1** (workspace `Cargo.toml` rendering-stack deps + `[patch.crates-io] bevy_winit` removal) is **shipped**, the **`patches/bevy_winit/` directory and `apps/tw-test/` are deleted**, and **Step 2** (`crates/ene-vrm` skeleton) and the **PR0 transparency smoke** that motivates the recipe are shipped. The rest of PR1 (steps 3–8: strip Bevy from the legacy `apps/ene-desktop`, move the source to v2, wire the winit window there, port the tray / AI bridge / settings) is still open. See §0 and §22.3 for the current state of v2.

**Objective:** Remove Bevy, get a transparent window with a clear color and a working system tray, with the AI bridge still hooked up but rendering nothing.

**Steps**

1. Edit the workspace `Cargo.toml`:
   - Add `[workspace.dependencies]`: `wgpu = "27"`, `winit = "0.30"`, `egui = "0.33"`, `egui-wgpu = "0.33"`, `egui-winit = "0.33"`, `glam = "0.29"`, `gltf = "1.4"`, `encase = "0.12"`, `bytemuck = "1"`, `pollster = "0.4"`, `raw-window-handle = "0.6"`.
   - Remove from `[workspace.dependencies]` and from `apps/ene-desktop/Cargo.toml`: `bevy`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1`, `bevy_animation`, `bevy_asset`, `bevy_render`, `bevy_math`, `bevy_mesh`, `bevy_window`, `bevy_input`, `bevy_image`, `bevy_transform`, `bevy_utils`, `bevy_ecs`.
   - Remove `[patch.crates-io] bevy_winit = { path = "patches/bevy_winit" }` and delete the `patches/bevy_winit` directory.
2. Create `crates/ene-vrm/` with `Cargo.toml`, `src/lib.rs` (just an empty `pub fn version() -> &'static str`), and add to workspace members (`crates/ene-vrm` is already covered by `crates/*`).
3. Replace `apps/ene-desktop/Cargo.toml` dependencies with the workspace ones.
4. Rewrite `apps/ene-desktop/src/main.rs`:
   - `tokio::runtime::Runtime::new()` and `runtime.enter()` retained.
    - Initialise `wgpu::Instance` (DX12 on Windows with `DxgiFromVisual`, Vulkan on Linux and macOS) and `wgpu::Adapter` + `wgpu::Device` + `wgpu::Queue` via `pollster::block_on` or a oneshot.
   - Build a `winit::EventLoop`, set `ControlFlow::Wait`, register the primary character window (`Arc<winit::Window>`) with `WindowAttributes` that mirror `window_plugin()`: `WindowLevel::AlwaysOnTop`, `transparent: true`, `decorations: false`, `resizable: true`, `inner_size: (320, 480)`.
    - On Windows, keep `std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "DxgiFromVisual")` **before** `Instance::new`; use DX12 backend.
   - On Linux, call `gtk::init()` early (tray).
   - Per-frame handler: redraw the primary window with `clear_color: (0,0,0,0)`, present, return.
5. Port the AI bridge: drop `bevy::Message`; introduce `ai_bridge::AiBridge { handle: EneHandle, receiver, processing: AtomicBool, pending: Mutex<VecDeque<EneStreamEvent>> }`. The bridge starts a background tokio task that pulls from `EneHandle::events()` and pushes into the deque.
6. Port the tray: keep `tray-icon` exactly as it is. The Windows message thread is independent of winit. On Linux, the GTK main thread still polls the tray; we just have to be careful that the winit event loop is created on the main thread (it is, since `gtk::init` does not steal main).
7. Port the settings window: **deferred to PR2**. For now, settings can be opened as a no-op toast or an unimplemented menu entry.
8. **Verification:** Manual smoke only. `cargo run -p ene-desktop` should show a transparent (or fully white) window that follows the cursor and re-creates on resize. No VRM, no settings yet.

**Files touched / created**

- `Cargo.toml` (workspace): dep changes
- `apps/ene-desktop/Cargo.toml`: dep changes
- `apps/ene-desktop/src/main.rs`: rewrite
- `apps/ene-desktop/src/{app_config,resources,ai_bridge,tray,platform}.rs`: rewrite (strip Bevy, keep public functions)
- `apps/ene-desktop/src/{scene,character,settings_ui,character_drag}.rs`: **delete** (PR1 is the deletion step; ported modules arrive in later PRs)
- `crates/ene-vrm/Cargo.toml` + `src/lib.rs`: new
- `patches/bevy_winit/`: delete

### PR2 — egui integration + settings window

**Objective:** Wire egui into the per-window renderer and bring the settings window back.

**Steps**

1. New module `apps/ene-desktop/src/runtime/window_slot.rs`:
   ```rust
   pub struct WindowSlot {
       pub window: Arc<winit::Window>,
       pub surface: wgpu::Surface<'static>,
       pub config: wgpu::SurfaceConfiguration,
       pub depth: wgpu::Texture,           // for PR3+; created lazily
       pub egui_ctx: egui::Context,
       pub egui_state: egui_winit::State,
       pub egui_renderer: egui_wgpu::Renderer,
   }
   ```
2. New module `apps/ene-desktop/src/runtime/surface.rs`: create / reconfigure `wgpu::Surface` on `Resized`, `ScaleFactorChanged`.
3. New module `apps/ene-desktop/src/runtime/input.rs`: route `winit::WindowEvent` to either `egui_state.on_window_event` (if the window has an egui context) or to app-level handlers (drag, hotkeys).
4. Render pipeline per window per frame:
   1. Update `egui_ctx` from input.
   2. Run a `ui::paint(window_id, &egui_ctx, &state.settings, &state.ai)` closure that fills an `egui::FullOutput`.
   3. `egui_ctx.tessellate(...)` → list of meshes.
   4. Encode a wgpu command encoder: clear with `(0,0,0,0)` for the character window, opaque for the settings window. Draw nothing in 3D. Then `egui_renderer.render(...)` blending over the cleared color.
5. New module `apps/ene-desktop/src/ui/`:
   - `mod.rs` — `pub fn paint(ctx: &egui::Context, settings: &mut CharacterSettings, ai: &mut AiBridge)`. Decides which page is shown.
   - `page_ai.rs`, `page_character.rs`, `page_graphics.rs`, `widgets.rs` — ported from `settings_ui/`.
6. Tray menu gains a "Settings" item that opens a second `winit::Window` (opaque) with its own `WindowSlot` and egui context.

**Verification**

- On Windows with `DxgiFromVisual`, egui panels appear at the right positions in both windows.
- On Linux X11, the character window is transparent; the settings window is opaque.
- Closing the settings window via its title bar removes its `WindowSlot` and disposes the surface.
- `cargo clippy --workspace -- -D warnings` is clean.

### PR3 — `ene-vrm` static rendering (MToon + skinning)

**Objective:** Load a `.vrm` and render it with a hand-written MToon WGSL shader.

**Subtasks**

1. **`crates/ene-vrm/src/loader.rs`** — use the `gltf` crate to read a `.vrm` (which is a glTF binary with `extensionsUsed: ["VRMC_vrm", …]`). Extract:
   - Skins, inverse-bind matrices, joints.
   - Meshes + primitive vertex/index buffers.
   - MToon material parameters from `KHR_materials_unlit` extension on each primitive's material.
   - Texture data (base color, normal, emission, shade, matcap, rim) — load as `Vec<Image>` and upload through `wgpu::Queue::write_texture`.
2. **`crates/ene-vrm/src/model.rs`** — public types:
   ```rust
   pub struct VrmModel {
       pub meshes: Vec<MeshGpu>,
       pub skeleton: Skeleton,
       pub materials: Vec<MToonMaterial>,
       pub textures: Vec<wgpu::Texture>,
       pub nodes: Vec<Node>,
       pub root: NodeIndex,
   }
   pub struct Skeleton { pub joints: Vec<Joint>, pub inverse_bind: Vec<Mat4> }
   pub struct Joint { pub node: NodeIndex, pub local_bind: Transform }
   ```
3. **`crates/ene-vrm/src/skeleton.rs`** — compute a flat `&[Mat4]` of current skinning matrices from a node hierarchy + an optional `AnimationSampler` (PR5+ leaves animation as identity skin).
4. **`crates/ene-vrm/src/mtoon.rs`** + `shaders/mtoon.wgsl`:
   - Uniforms per draw: `MToonUniform { base_color, shade_color, shading_shift, … }`.
   - Bind groups: `(0)` skinning storage buffer, `(1)` material UBO, `(2)` per-frame camera.
   - Fragment implements the MToon lighting model (lit / shade / rim / matcap / outline). Outline pass is a separate render pass with `cull_mode: Front` and an inflated vertex along the normal.
5. **`crates/ene-vrm/src/renderer.rs`**:
   ```rust
   pub struct VrmRenderer {
       pipeline: wgpu::RenderPipeline,
       outline_pipeline: wgpu::RenderPipeline,
       skin_buf: wgpu::Buffer,
       camera_buf: wgpu::Buffer,
       camera_bgl: wgpu::BindGroupLayout,
   }
   impl VrmRenderer {
       pub fn new(device, surface_format) -> Self;
       pub fn render(
           &self,
           encoder: &mut wgpu::CommandEncoder,
           view: &wgpu::TextureView,
           depth: &wgpu::TextureView,
           model: &VrmModel,
           camera: &CameraUniform,
       );
   }
   ```
6. **`apps/ene-desktop/src/character/mod.rs`** — load default VRM, store in `WindowSlot::vrm_model`, drive `VrmRenderer::render` between the clear and the egui pass.
7. The render order per frame becomes: clear → outline pass → main pass → egui.

**Verification**

- Drop in a known-good `.vrm` (e.g. the bundled sample) and see it standing in the window with the camera in front of it.
- Drag the window; the camera distance updates; the model stays centered.
- `cargo test -p ene-vrm` (new unit tests for loader and skeleton math).

### PR4 — Expressions, LookAt, BodyTracking

**Objective:** Make the character react to the cursor and to AI-emitted emotions.

**Subtasks**

1. **Expression** — port `bevy_vrm1::vrm::expression`:
   - Each frame, build a `BTreeMap<ExpressionName, f32>` from the latest `EmotionQueue` (driven by `ai_bridge`).
   - Multiply the per-expression weight into the per-primitive morph-target buffer.
   - Public API: `VrmModel::set_expression(name, weight)`, `VrmModel::expression_names()`.
2. **LookAt** — port `bevy_vrm1::vrm::body_tracking` (only the look-at part for now):
   - Provide a `LookAtTarget { world_position: Vec3 }` per frame.
   - Solve two-bone IK on the spine → head → eyes chain to point the eyes at the target.
   - Clamp yaw / pitch to the model's VRM-defined ranges.
   - In `apps/ene-desktop/src/character/cursor.rs`, convert the OS cursor position to world coordinates in front of the camera at a fixed depth and feed it in.
3. **BodyTracking** — keep a minimal version: only head + eyes follow cursor; shoulder / hand sway is out of scope until we re-add spring bone.
4. **AI bridge integration** — `AiBridge::drain()` is called from `runtime` once per frame; resulting `EmotionQueue` is fed to `VrmModel::apply_emotions`.

**Verification**

- Move the OS cursor: the model's head / eyes track it within the configured clamp angles.
- Type "I'm so happy!" in the chat: the model transitions to a happy blend shape.
- Add an automated test in `ene-vrm` that loads a tiny synthetic VRM, sets an expression, and asserts the morph-target buffer reflects the weight.

### PR5+ — Deferred Work

- **PR5** — VRMA playback (cloning `bevy_vrm1::vrma`).
- **PR6** — Spring-bone (cloth / hair) — full port of `bevy_vrm1::vrm::spring_bone`.
- **PR7** — Shadow quality switching (FXAA / SMAA / TAA) and a settings UI page to control it.
- **PR8** — Drag-while-clicked improvements (smoother multi-monitor handling).

Each PR is its own design doc snippet; we add a `## Open Follow-ups` block at the end of this file when PR3 lands.

---

## 5. New / Removed Files (summary)

> **Status as of writing:** Only the **bold** items below exist on disk today. Everything else is planned and lands in the corresponding PR. The PR0-specific reality (3 files in `apps/ene-desktop-v2/` instead of the 7-module split originally sketched here) is documented in §22.3.

### 5.1 New top-level

- **`crates/ene-vrm/Cargo.toml`** (PR1 step 2, shipped)
- **`crates/ene-vrm/src/lib.rs`** (PR1 step 2, shipped — stub only)
- `crates/ene-vrm/src/loader.rs` (PR3)
- `crates/ene-vrm/src/model.rs`
- `crates/ene-vrm/src/skeleton.rs`
- `crates/ene-vrm/src/mtoon.rs`
- `crates/ene-vrm/src/expression.rs`
- `crates/ene-vrm/src/look_at.rs`
- `crates/ene-vrm/src/camera.rs`
- `crates/ene-vrm/src/renderer.rs`
- `crates/ene-vrm/src/spring_bone.rs` (skeleton in PR5)
- `crates/ene-vrm/src/vrma.rs` (skeleton in PR5)
- `crates/ene-vrm/src/shaders/mtoon.wgsl`
- `crates/ene-vrm/src/shaders/outline.wgsl`
- `crates/ene-vrm/src/shaders/sky.wgsl`
- `crates/ene-vrm/tests/loader.rs`
- `crates/ene-vrm/tests/skeleton.rs`

### 5.2 New in `apps/ene-desktop`

- `src/main.rs` (rewrite)
- `src/app_config.rs` (rewrite without Bevy)
- `src/resources.rs` (no change in behaviour)
- `src/ai_bridge.rs` (rewrite without Bevy)
- `src/tray.rs` (no change in behaviour, signature cleanup)
- `src/drag.rs` (move from `character_drag/mod.rs`)
- `src/runtime/mod.rs`
- `src/runtime/window_slot.rs`
- `src/runtime/surface.rs`
- `src/runtime/input.rs`
- `src/runtime/loop.rs`
- `src/gpu/mod.rs`
- `src/gpu/depth.rs`
- `src/gpu/camera.rs`
- `src/platform/mod.rs`
- `src/platform/windows_hwnd.rs`
- `src/platform/drag_subclass.rs`
- `src/platform/wayland_layer.rs`
- `src/ui/mod.rs`
- `src/ui/page_ai.rs`
- `src/ui/page_character.rs`
- `src/ui/page_graphics.rs`
- `src/ui/widgets.rs`
- `src/character/mod.rs`
- `src/character/cursor.rs`
- `src/character/drag.rs`

### 5.2b New in `apps/ene-desktop-v2` (PR0, shipped)

This is the **actual** file layout on disk today. The full split below lands incrementally as PR1–PR5 progress; once the migration is done the v2 crate will be moved to `apps/ene-desktop` and these files will be replaced by the §5.2 layout.

- **`apps/ene-desktop-v2/Cargo.toml`** — slimmed deps: `winit`, `wgpu`, `pollster`, `bytemuck`, `glam`, `tracing`, `tracing-subscriber`. No `raw-window-handle`, `windows-sys`, `ene-core`, `tokio`, `egui`, or `tray-icon` yet.
- **`apps/ene-desktop-v2/src/main.rs`** — `tracing_subscriber::fmt` install + `EventLoop::run_app`.
- **`apps/ene-desktop-v2/src/gpu.rs`** — `GpuContext`, `pick_format_and_alpha`, `backend_options` (DX12 / `DxgiFromVisual` on Windows, `Backends::PRIMARY` elsewhere).
- **`apps/ene-desktop-v2/src/runtime.rs`** — `Runtime`, `WindowSlot`, `RectRenderer`, `ApplicationHandler` impl, `AcquireError`. Input handling inlined as match arms; no separate `input.rs` / `surface.rs` / `rect.rs` modules.

### 5.3 Removed

- **`patches/bevy_winit/`** (entire directory — done; the `[patch.crates-io] bevy_winit` entry in the workspace `Cargo.toml` is also gone)
- `apps/ene-desktop/src/scene.rs`
- `apps/ene-desktop/src/character.rs`
- `apps/ene-desktop/src/settings_ui/` (replaced by `src/ui/`)
- `apps/ene-desktop/src/character_drag/` (logic moves to `src/platform/drag_subclass.rs`)
- `apps/ene-desktop/src/platform.rs` (split into `src/platform/`)
- **`apps/tw-test/`** (the Bevy transparency testbed — done; the cross-reference in §22.3 is preserved as a historical note only)

---

## 6. Dependency Changes

> **Status as of writing:** The **"Done"** column below reflects the current workspace. `Added` is half-done (all workspace deps declared; only `apps/ene-desktop-v2` consumes them). `Removed` is partly done: the local `bevy_winit` patch is gone (the legacy `apps/ene-desktop` now uses upstream `bevy_winit` 0.18 from crates.io), but the rest of the Bevy stack is still in `apps/ene-desktop/Cargo.toml`.

### 6.1 Added (workspace)

| Crate | Version | Why | Done? |
|-------|---------|-----|-------|
| `wgpu` | 27 | wgpu core (matches wgpu 27 used in Bevy 0.18) | yes (workspace dep; consumed by `apps/ene-desktop-v2`) |
| `winit` | 0.30 | event loop and windowing | yes (workspace dep; consumed by `apps/ene-desktop-v2`) |
| `egui` | 0.33 | immediate-mode UI | workspace dep declared, not yet consumed (PR2) |
| `egui-wgpu` | 0.33 | egui → wgpu renderer | workspace dep declared, not yet consumed (PR2) |
| `egui-winit` | 0.33 | egui input integration | workspace dep declared, not yet consumed (PR2) |
| `glam` | 0.29 | linear algebra | yes (consumed by `apps/ene-desktop-v2`) |
| `gltf` | 1.4 | VRM / glTF parsing | workspace dep declared, not yet consumed (PR3) |
| `encase` | 0.12 | shader-compatible struct packing (UBOs) | workspace dep declared, not yet consumed (PR3) |
| `bytemuck` | 1 | safe `Pod`/`Zeroable` casts | yes (consumed by `apps/ene-desktop-v2`) |
| `pollster` | 0.4 | minimal `block_on` for startup | yes (consumed by `apps/ene-desktop-v2`) |
| `raw-window-handle` | 0.6 | wgpu surface creation | workspace dep declared, not yet consumed (PR3) |

### 6.2 Kept

- All `ene-core`, `ene-memory`, `ene-config`, `ene-provider`, `ene-embedding`, `ene-session`, `ene-tool-*`, `ene-tool-host`, `ene-tool-proto`, `ene-tool-derive`, `ene-common` deps.
- `tray-icon` (used directly in `tray.rs`).
- `tokio`, `serde`, `serde_json`, `figment`, `anyhow`, `thiserror`, `tracing`, `directories`.

### 6.3 Removed (workspace)

> **Partly done.** The patch line is gone and the legacy build now uses upstream `bevy_winit`. The Bevy crates themselves are still in `apps/ene-desktop/Cargo.toml` (the legacy binary is untouched until PR1 step 3).

- `bevy`, `bevy_ecs`, `bevy_pbr`, `bevy_winit`, `bevy_egui`, `bevy_vrm1`, `bevy_animation`, `bevy_asset`, `bevy_render`, `bevy_math`, `bevy_mesh`, `bevy_window`, `bevy_input`, `bevy_image`, `bevy_transform`, `bevy_utils`.
- ~~`[patch.crates-io] bevy_winit` and the `patches/bevy_winit/` directory~~ **(done — both deleted)**.

---

## 7. Windowing, Surface, and Event Loop

### 7.1 Event loop

- `winit::event_loop::EventLoop::new()` with `EventLoop::run` on the main thread.
- `control_flow: ControlFlow::Wait` for low CPU when idle; `WindowEvent::RedrawRequested | WindowEvent::Resumed` flip to `Poll` for one tick.
- All `WindowId` keys come from winit; we keep `Arc<winit::Window>` and the corresponding `wgpu::Surface<'static>` together in a `WindowSlot`.
- We **do not** create the wgpu device inside `Resumed`; we create it once at startup using `pollster::block_on` and rely on `wgpu`'s reconfigure path on `Resized`.

### 7.2 Surface configuration

```rust
fn configure(surface: &wgpu::Surface, device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied, // windows; Auto on linux
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    }
}
```

Per platform:

- **Windows (DX12)** — `format = device.adapter.get_supported_surface_formats(...).first()`. With `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual`, `CompositeAlphaMode::PreMultiplied` is supported and gives true per-pixel alpha. (Bug B1 fixed because we now own the surface / swapchain code path.)
- **Linux + X11 (Vulkan)** — wgpu reports `CompositeAlphaMode::PreMultiplied` only if the X11 visual is ARGB. We pick a 32-bit RGBA visual when creating the winit window. If unavailable, fall back to `Auto` and document the limitation.
- **Linux + Wayland** — See §10.

### 7.3 Per-frame encoder

```rust
let frame = slot.surface.get_current_texture()?;
let view = frame.texture.create_view(&Default::default());
let mut encoder = device.create_command_encoder(&Default::default());
{
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("frame"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0., g: 0., b: 0., a: 0. }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None, // PR3
        timestamp_writes: None,
    });
    // 3D draws (PR3+)
    drop(rp);
}
{
    let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { /* same view, load Op::Load, no clear */ });
    // egui draws
    drop(rp);
}
queue.submit([encoder.finish()]);
frame.present();
```

The second pass uses `LoadOp::Load` so egui blends on top of whatever the 3D pass produced.

---

## 8. Transparency Strategies (per platform)

### 8.1 Windows

- `WGPU_DX12_PRESENTATION_SYSTEM=DxgiFromVisual` (set in `main.rs` before `Instance::new`).
- `WindowAttributes::default().with_transparent(true)`.
- `CompositeAlphaMode::PreMultiplied`.
- Drag-through: keep the existing `SetWindowSubclass` implementation from `character_drag/windows.rs`, moved to `src/platform/drag_subclass.rs`. It toggles `WS_EX_TRANSPARENT` on `WM_NCHITTEST`.

### 8.2 Linux + X11 (Vulkan)

- Force `wgpu::Backends::VULKAN`.
- Force a 32-bit RGBA visual when creating the winit window. We use the `x11rb` crate to read the screen's ARGB visual list and pick one. (If winit's `WindowBuilder` cannot pick an ARGB visual, we open the X11 window manually through x11rb and hand the raw handle to winit via `WindowAttributes::with_window`.)
- Set `_NET_WM_WINDOW_OPACITY` via `x11rb` to `0xFFFFFFFF` so the compositor does not flatten alpha to zero.
- Drag-through: use `shape` extension or `_NET_WM_WINDOW_OPACITY` per-pixel via a 1-bit mask; default is no drag-through on X11, and we ship a TODO for clip-shape in a later PR.

### 8.3 Linux + Wayland

- Force `wgpu::Backends::VULKAN`.
- Use `smithay-client-toolkit` (`sctk`) to negotiate a `zwlr-layer-shell-v1` surface when the compositor supports it. The layer is `Layer::Overlay` so the character can float above fullscreen windows.
- `sctk::layer::LayerSurface::with_alpha(0.0)` requests per-pixel alpha; `sctk::shell::wlr_layer::Anchor::empty()` plus `KeyboardInteractivity::None` makes it click-through by default.
- Drag is implemented by handling `pointer_motion` and `pointer_button` from the layer surface. We track the delta in the app state and update window position via `layer_surface::Surface::commit()`.
- Fallback: if `zwlr-layer-shell-v1` is not advertised, fall back to a normal `xdg-shell` window. Drag-through is then limited to a global hotkey "freeze character window" toggle.

### 8.4 macOS

- Force `wgpu::Backends::VULKAN`.
- `WindowAttributes::with_transparent(true)` and `CompositeAlphaMode::PreMultiplied`. We do not test or document behaviour.

---

## 9. egui Integration

### 9.1 One context per window

- The character window has its own `egui::Context` that we **do not use** for now (kept for future debug overlays).
- The settings window has its own `egui::Context`. All settings pages render into it.
- A "Character" debug toggle in the settings page (planned) will pop an egui overlay over the character window using its own context.

### 9.2 Frame pump

```rust
fn pump_egui(slot: &mut WindowSlot, state: &mut AppState) -> egui::FullOutput {
    let raw = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(
            Pos2::ZERO,
            vec2(slot.config.width as f32, slot.config.height as f32),
        )),
        pixels_per_point: Some(slot.window.scale_factor() as f32),
        time: Some(state.now),
        ..slot.egui_state.take_egui_input(&slot.window)
    };
    slot.egui_ctx.run(raw, |ctx| ui::paint(ctx, &mut state.settings, &mut state.ai))
}
```

### 9.3 Render into the same surface

After the 3D pass (§7.3) we start a new render pass with `LoadOp::Load` and call `egui_wgpu::Renderer::render` with the `egui::PaintJobs` returned by `egui::Context::tessellate`.

### 9.4 Hotkey for the settings window

- Register a global hotkey (e.g. `Ctrl+,`) through `winit::EventLoop::run` (or a platform-specific API on Windows: `RegisterHotKey`; on Linux: a `GlobalShortcutsPortal` zbus call). The hotkey toggles a `bool` in `AppState`; on the next frame, the runtime creates or destroys the settings `WindowSlot`.

---

## 10. `ene-vrm` Crate Internals

### 10.1 Public API (sketch)

```rust
pub struct VrmHandle(/* Arc<VrmModelInner> */);

pub struct VrmRenderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipelines: MtoonPipelines,
    skin_buf: wgpu::Buffer,
    camera_buf: wgpu::Buffer,
    bind_layouts: BindLayouts,
}

impl VrmRenderer {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>, format: wgpu::TextureFormat, depth_format: wgpu::TextureFormat) -> Self;
    pub fn load(&self, path: impl AsRef<Path>) -> Result<VrmHandle, VrmError>;
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        model: &VrmHandle,
        camera: &CameraUniform,
        expressions: &ExpressionState,
        look_at: Option<LookAtTarget>,
    );
}

#[derive(Copy, Clone, encase::ShaderType)]
pub struct CameraUniform {
    pub view: glam::Mat4,
    pub proj: glam::Mat4,
    pub view_proj: glam::Mat4,
    pub eye: glam::Vec3,
}

#[derive(Default)]
pub struct ExpressionState(pub BTreeMap<String, f32>);

pub struct LookAtTarget {
    pub world_position: glam::Vec3,
}
```

### 10.2 Loader (gltf)

`gltf::Importer::import_path` is sync. We wrap it in `spawn_blocking` from the AI runtime when loading a brand-new model. For the bundled default VRM, we load it at startup before the winit event loop begins (so we can size the window to the model's AABB).

Reading the glTF:

- Walk `document.skins()` → `Skeleton::from_skin(&skin)`.
- Walk `document.nodes()` recursively, build a flat `Vec<Node>` with `transform: Transform`, `children: Vec<NodeIndex>`, `mesh: Option<usize>`, `skin: Option<usize>`. We treat the node that is the root of a humanoid (VRMC_vrm-1.0 humanoid bone `hips`) as the root.
- Walk `document.meshes()` → upload `Primitive { vertex_buffer, index_buffer, material_index }`. The vertex layout is the **MToon-required** layout: position, normal, tangent (optional), uv0, color, **joints (uvec4)**, **weights (vec4)**. We pre-pad to 4 joints per vertex; extras go to weight 0.
- Walk `document.materials()` and the `KHR_materials_unlit` extension, mapping its `pbrMetallicRoughness.baseColorTexture` etc. to `MToonUniform`.

We use `gltf::image::Data` to get raw RGBA / BCn bytes and upload them with `wgpu::util::DeviceExt::create_texture_with_data` to avoid manual staging copies.

### 10.3 MToon shader

`shaders/mtoon.wgsl`:

- **Vertex** — read `joints`, `weights`; compute `skinned_position = sum(weight[i] * skin[i] * position)` and `skinned_normal = …`; pass through UV, color.
- **Fragment** — implements the VRM MToon lighting model in 0..1 space:
  - `lit_factor` from `dot(N, L)` raised to `shading_shift`.
  - Mix between `base_color` and `shade_color` by `lit_factor` with `shade_toony` smoothstep.
  - Specular: GGX-lite with `parametric.ramp` lookup.
  - Emission: `emissive` + emission map.
  - Outline: a separate vertex stream that displaces along the normal by `outline_width * (1.0 - lit_factor)`. Pass is `cull_mode: Front`, `depth_write: false`, `blend: (SrcAlpha, OneMinusSrcAlpha)`.
  - Output is `vec4(premultiplied_color, alpha)`. We use `premultiplied_alpha` output so the window-level `CompositeAlphaMode::PreMultiplied` and the MToon output are consistent.

We start with a single-directlight model; we add an environment term (matcap / rim) in a follow-up. The shader is annotated with `// SPEC: MToon 1.0 §3.4.1` etc. so reviewers can compare against the official spec.

### 10.4 Expression / Morph

- The glTF mesh primitive's `targets` field carries per-vertex deltas for blend shapes.
- We allocate one storage buffer per mesh: `morph_offsets: array<vec3>` indexed by `[primitive_index][target_index]`.
- For each expression name (e.g. `joy`, `blink`, `aa`), we look up the corresponding glTF target by reading VRMC_vrm-1.0's `blendShapeMaster`. We pre-allocate the `ExpressionState` BTreeMap with the names found.
- The vertex shader takes `morph_weights: array<vec4>` packed 4-at-a-time and `morph_offset_count: u32`. It applies `position += sum(weight[i] * offsets[i])`.

### 10.5 LookAt

- VRM 1.0 has a `lookAt` object on the meta. We read:
  - `lookAtType` (`bone` or `expression`).
  - `rangeMapHorizontalInner` / `rangeMapHorizontalOuter` / vertical equivalents (used to clamp yaw / pitch).
- For `bone` mode, we walk the humanoid bone hierarchy (`head`, `leftEye`, `rightEye`) and apply a small quaternion delta. The `lookAt` system re-uses `bevy_vrm1`'s `compute_look_at_slerp` idea but with `glam::Quat::slerp`.
- For `expression` mode, we feed `ExpressionState` with `lookLeft`, `lookRight`, `lookUp`, `lookDown` weights derived from the target direction. No bone math.

### 10.6 Body tracking (cursor follow)

- `apps/ene-desktop/src/character/cursor.rs` converts the OS cursor screen position to a `Vec3` in front of the camera at a fixed depth (1.5 m) and passes it as `LookAtTarget`.
- `ene-vrm` translates the world position into model-local space, calls `solve_look_at` (PR4), and returns the new head / eye transforms. We do not modify spine or shoulders in PR4.

### 10.7 Tests (in `crates/ene-vrm/tests/`)

- `loader.rs` — load a synthetic minimal glTF (built with `gltf-json` in a helper) and assert skinning matrices, joint count, and material parameters.
- `skeleton.rs` — known-bone rest pose; assert skin matrices.
- `expression.rs` — apply a weight of 1.0 to a known blend shape; assert the offset buffer length and a sample offset.
- `look_at.rs` — given a head bone, look at a target; assert the resulting head forward is within ε of the normalized direction.

Tests run on any platform because they don't need a real GPU: we use `wgpu::Device::from_buffer` is not possible, so we use a **headless** test device via `wgpu::util::initialize_adapter_from_env_or_default` with `wgpu::Features::empty()` and `wgpu::Limits::downlevel_defaults()`. On CI without a GPU we skip these tests with `#[ignore]` per AGENTS.md.

---

## 11. AI Bridge Refactor

`apps/ene-desktop/src/ai_bridge.rs` becomes:

```rust
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use ene_core::api::{EneEvent, EneHandle};

#[derive(Clone)]
pub struct AiBridge {
    handle: EneHandle,
    inner: Arc<Inner>,
}

struct Inner {
    receiver: tokio::sync::mpsc::UnboundedReceiver<EneEvent>,
    processing: AtomicBool,
    pending: Mutex<VecDeque<EneEvent>>,
    emotion_queue: Mutex<VecDeque<EmotionSample>>,
}

impl AiBridge {
    pub fn spawn(handle: EneHandle) -> Self { /* start task */ }
    pub fn is_processing(&self) -> bool { self.inner.processing.load(Ordering::Acquire) }
    pub fn drain(&self) -> Vec<EneEvent> {
        std::mem::take(&mut *self.inner.pending.lock().unwrap())
    }
    pub fn latest_emotion(&self) -> Option<EmotionSample> { /* pop from queue */ }
}
```

The background task is a `tokio::spawn` on the desktop runtime:

```rust
async fn pump(mut receiver: mpsc::UnboundedReceiver<EneEvent>, inner: Arc<Inner>) {
    while let Some(ev) = receiver.recv().await {
        match &ev {
            EneEvent::RunStart => inner.processing.store(true, Ordering::Release),
            EneEvent::RunEnd { .. } => inner.processing.store(false, Ordering::Release),
            _ => {}
        }
        inner.pending.lock().unwrap().push_back(ev);
    }
}
```

The runtime calls `ai_bridge.drain()` once per frame and forwards events to `ui::paint` (text deltas, run boundaries) and `character` (emotions).

---

## 12. System Tray, Hotkeys, Settings Persistence

- **Tray** — `tray-icon` 0.x. On Windows we keep the dedicated `GetMessage` thread; on Linux we use the GTK main loop. The tray menu is rebuilt from a closure that captures `Arc<AppState>` so "Settings", "Reload VRM", "Quit" remain functional.
- **Hotkeys** — On Windows we use `RegisterHotKey` (moved from `character_drag/windows.rs`). On Linux we use the `org.freedesktop.portal.GlobalShortcuts` zbus interface; we keep the existing `zbus` dep that was added for the desktop portal but was unused.
- **Settings persistence** — `ene_config::CharacterSettings` is read once at startup, written back from the settings window's egui page. We keep this exactly as it is.

---

## 13. Drag-While-Clicked and Window Passthrough

### 13.1 Windows

- `platform::drag_subclass` registers a window subclass on creation. The subclass handles `WM_NCHITTEST` to return `HTTRANSPARENT` when the user holds the configured modifier (default: right mouse button). The rest of the input goes to the winit event loop.
- The subclass also installs a low-level mouse hook (`SetWindowsHookExW(WH_MOUSE_LL, …)`) only when the user is dragging, to keep the modifier state globally accessible.

### 13.2 Linux

- **X11** — `_NET_WM_WINDOW_OPACITY` is set globally; for true hit-test passthrough we use the `shape` extension. We will ship a no-op stub in PR1 and enable it in PR8.
- **Wayland** — `pointer_enter` / `pointer_leave` events on the `sctk` surface let us decide whether to grab the pointer. Drag is implemented via a modifier-aware region in the surface (a 1-pixel transparent ring is not feasible on Wayland, so we use the layer-shell "interactive region" if the compositor supports it; otherwise we fall back to a global modifier key).

---

## 14. Configuration & Schemas

- `assets/character_settings.schema.json` and `assets/settings.schema.json` continue to be auto-regenerated by the `define_config!` macro in `ene-config`. We do not change the schema in this work.
- We add one new top-level key: `vrm.file` (already exists, kept as-is).
- For graphics, we simplify to a single `vulkan_forced: bool` (default `false` on Windows, `true` on Linux). FXAA / SMAA / TAA toggles are removed in PR1; we keep `vrm.outline_width` and `vrm.look_at_clamp_*` for PR4.

---

## 15. Error Handling, Logging, and Panics

- All wgpu errors go through a single `wgpu::ErrorScope` wrapped by `tracing::error!`. We never `unwrap()` a `RequestDeviceError` or `SurfaceError`.
- `wgpu::SurfaceError::Lost | Outdated | OutOfMemory` triggers a `slot.recreate_surface()` (recreate the wgpu surface from the same `Arc<Window>`).
- The tray thread (Windows) and the GTK thread (Linux) never panic. They communicate back to the runtime via `mpsc::Sender<AppEvent>` (`AppEvent::TrayAction(TrayAction)`, `AppEvent::Quit`).
- The AI bridge task is the only long-running tokio task on the desktop runtime. On panics it logs and restarts up to 3 times; after that, the tray's status icon flips to red and the user is asked to quit.

---

## 16. Testing Strategy

### 16.1 Unit tests

- `crates/ene-vrm` — loader, skeleton, expression, look_at (see §10.7).
- `apps/ene-desktop/src/runtime/surface.rs` — surface configuration logic with a `mock_device()` helper.
- `apps/ene-desktop/src/ai_bridge.rs` — channel + drain semantics (no GPU needed).

### 16.2 Integration tests

- A new `crates/ene-vrm/tests/gltf_roundtrip.rs` uses `gltf-json` to construct an in-memory glTF, writes it to a `tempfile::TempDir`, and asserts `VrmRenderer::load` succeeds.
- `apps/ene-desktop/tests/window_lifecycle.rs` is `#[ignore]`d; it requires a real display and is run manually on Linux.

### 16.3 Manual smoke checklist (per PR)

PR1:
- [ ] App starts on Windows with `DxgiFromVisual` and a transparent window is visible.
- [ ] App starts on Linux + X11 + Vulkan; window is visible.
- [ ] App starts on Linux + Wayland (GNOME / KDE) + Vulkan; window is visible.
- [ ] Tray icon appears, "Quit" works.
- [ ] `cargo clippy --workspace -- -D warnings` is clean.

PR2:
- [ ] Settings window opens via tray menu, renders, closes cleanly.
- [ ] Closing the settings window does not affect the character window.
- [ ] Resizing the settings window recomposes correctly.

PR3:
- [ ] Default VRM renders. Camera distance is sane. Window resize re-renders correctly.
- [ ] No GPU validation warnings (`wgpu` reports none).
- [ ] `cargo test -p ene-vrm` passes.

PR4:
- [ ] Cursor movement is reflected in head/eye direction.
- [ ] AI emotion text → blend shape transition is smooth (no single-frame snaps).
- [ ] Multiple successive emotion inputs compose correctly.

### 16.4 CI

- `cargo fmt --all --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace` (the GPU tests are `#[ignore]`d)
- `cargo build --workspace --release`

No display-server dependent step runs in CI for now; we add a Wayland/X11 smoke as a separate workflow when we have a runner.

---

## 17. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `DxgiFromVisual` is not supported on a particular Windows GPU driver | Low | High (PR1 cannot be verified) | Detect at startup; if absent, fall back to `Auto` and log a warning. The fix is to update the driver or upgrade wgpu. |
| Wayland compositor does not advertise `zwlr-layer-shell-v1` (e.g. older Sway) | Medium | Medium | Fall back to `xdg-shell` and document the limitation. |
| MToon shader path diverges from bevy_vrm1's visuals | High | Medium | Side-by-side render tests against the existing app before PR3 is merged. Use the same default VRM. |
| `wgpu` re-exports change minor versions in 0.x releases | Medium | Low | Pin to `wgpu = "27.0.1"` exactly until 27.x is stable. |
| `egui-wgpu` 0.33 API differs from 0.39 used in `bevy_egui` | Medium | Low | Examples in the egui-wgpu repo are the source of truth. We write a small `EgUiWgpuHelper` once. |
| `bevy_winit` patch removal breaks something in unrelated tooling | Low | Low | The patch only affects `bevy_winit`. Other tooling doesn't depend on it. |

---

## 18. Open Questions / Future Work

- **Q1.** Should we use `naga-oil` to compose shaders or keep raw WGSL files? — Default: raw WGSL, optional include path support via `naga-oil` later.
- **Q2.** Should the settings window share the egui context with the character window? — No (we keep them isolated for clarity and to allow future per-window styling).
- **Q3.** Should we expose `ene-vrm` as a wasm-bindgen target? — Defer; the MToon shader is portable but the wgpu device selection on wasm is non-trivial.
- **Q4.** Spring bone step count — bevy_vrm1's default is `8` iterations / 60 Hz. We will expose both as `vrm.spring_bone.*` in PR6.
- **Q5.** VRMA — after PR5, decide whether to keep a custom VRMA parser or depend on the official `vrma` crate. None exists yet; we will write our own.

---

## 19. Glossary

- **VRM** — glTF-based avatar format. v1.0 uses the `VRMC_vrm-1.0` extension. v0.x uses `VRMC_vrm-0.x` and is **out of scope** for this migration.
- **MToon** — the cel-shading material defined by VRM. Inputs: base color, shade color, shading shift, rim, matcap, outline.
- **Pre-multiplied alpha** — output color `(R*A, G*A, B*A, A)`; needed for wgpu's `CompositeAlphaMode::PreMultiplied` swapchain to honor per-pixel alpha.
- **DxgiFromVisual** — wgpu's DX12 backend option that creates the swapchain from the HWND's visual, required for `WS_EX_LAYERED` per-pixel alpha.
- **layer-shell** — Wayland protocol for desktop overlay windows (used by Waybar, etc.). Provides real per-pixel alpha and `Above` layering.
- **Spring bone** — VRM 0.x / 1.x secondary motion for hair, cloth, etc. Driven by a simple spring-damper simulation.
- **VRMA** — VRM Animation, a glTF-based animation clip format.

---

## 20. Appendix: Mapping Bevy concepts to the new stack

| Bevy 0.18 | New code |
|-----------|----------|
| `App`, `DefaultPlugins` | `runtime::AppState` + manual `winit::EventLoop` |
| `Window`, `WindowPlugin` | `winit::window::Window` + `runtime::window_slot::WindowSlot` |
| `RenderDevice`, `RenderQueue` | `wgpu::Device`, `wgpu::Queue` in `gpu::Context` |
| `Camera3d` | `gpu::camera::Camera` (orthographic for now) |
| `Mesh`, `MeshPlugin` | `ene_vrm::model::MeshGpu` |
| `StandardMaterial` | `ene_vrm::mtoon::MToonMaterial` |
| `EguiPlugin` | `ui::paint` + `runtime::window_slot::WindowSlot` egui fields |
| `EguiMultipassSchedule` | the per-window "render egui after 3D" loop in `runtime::loop` |
| `VrmPlugin` | `ene_vrm::VrmRenderer` (constructed once) |
| `VrmaPlugin` | PR5+ |
| `Messages<T>` | `tokio::sync::mpsc` + `Arc<Mutex<VecDeque>>` |
| `Resource<T>` | `AppState` field |
| `Local<T>` | per-`WindowSlot` field |
| `Query<>` | direct field access on `WindowSlot` |
| `Time<()>` | `std::time::Instant` in `AppState` |
| `WinitSettings::desktop_app()` | `EventLoop::set_control_flow(ControlFlow::Wait)` + `set_redraw_requested` |
| `bevy::tasks::TaskPool` | `tokio::runtime` (already used) |
| `bevy::log` | `tracing` (already used) |

---

## 21. Sign-off Checklist (per PR)

- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --workspace --release`
- [ ] Manual smoke on Windows + Linux + Wayland for that PR's scope
- [ ] English docs updated under `docs/`
- [ ] Japanese docs updated under `docs/ja/`
- [ ] `docs/architecture/wgpu-migration.md` updated (this file) to reflect the new state

---

## 22. PR1 Implementation Notes

### 22.1 Real-world Windows gotcha: `Opaque`-only surfaces — **superseded by §22.3**

> **Superseded by PR0 / §22.3.** Every "correct diagnosis" in this section is itself out of date; the actual root cause is that wgpu 27 does not auto-read the `WGPU_DX12_PRESENTATION_SYSTEM` env var. The application has to pass the desired `Dx12SwapchainKind` directly to `BackendOptions::dx12::presentation_system` in `wgpu::Instance::new` (v2 does this in `apps/ene-desktop-v2/src/gpu.rs::backend_options`; Bevy 0.18 does the equivalent in `bevy_render/src/renderer/mod.rs:201`). See §22.3 for the full story and the working recipe.

The earlier PR1 attempts (DX12 env var + `WS_EX_LAYERED` nudge, Vulkan swap) and the intermediate "missing `WS_EX_NOREDIRECTION_BITMAP`" diagnosis are all preserved in git history but no longer relevant to anyone reading this file. Kept as a one-line pointer so cross-references from outside the doc do not 404.

### 22.2 PR1 file-level status (as of writing)

| Action | File / directory | State |
|--------|------------------|-------|
| **New** | `crates/ene-vrm/Cargo.toml` | Shipped (PR1 step 2) |
| **New** | `crates/ene-vrm/src/lib.rs` | Shipped (PR1 step 2 — `pub fn version()` stub + one unit test) |
| **New** | `apps/ene-desktop-v2/Cargo.toml` | Shipped (PR0) |
| **New** | `apps/ene-desktop-v2/src/{main,gpu,runtime}.rs` | Shipped (PR0 — full v2 smoke) |
| **Rewritten** | `apps/ene-desktop/src/{main,app_config,ai_bridge,tray}.rs` | **Not started** — legacy Bevy binary untouched |
| **Rewritten** | `apps/ene-desktop/Cargo.toml` | **Not started** — still Bevy 0.18 |
| **Deleted** | `apps/ene-desktop/src/{scene,character,platform}.rs` | **Not started** |
| **Deleted** | `apps/ene-desktop/src/{settings_ui,character_drag}/` | **Not started** |
| **Deleted** | `patches/bevy_winit/` | **Shipped** — patch directory and `[patch.crates-io] bevy_winit` in workspace `Cargo.toml` are both gone; the legacy `apps/ene-desktop` now uses upstream `bevy_winit` 0.18 from crates.io |
| **Deleted** | `apps/tw-test/` | **Shipped** — Bevy testbed removed; the §22.3 cross-reference is preserved as a historical note only |
| **Workspace** | `Cargo.toml` rendering-stack section | **Partially** — deps added (PR0) but `[patch.crates-io] bevy_winit` not yet removed |

The high-level intent ("PR1 is the deletion step") is unchanged; only the rows that PR0 advanced have been marked Shipped.

### 22.3 PR0 — Minimum v2 transparency smoke

> **Status:** Shipped. This is the smallest possible `apps/ene-desktop-v2` that proves the §8.1 Windows transparency recipe works end-to-end on this machine. It supersedes the (incorrect) §22.1 diagnosis.

**v2 layout (3 files, ~330 lines total):**

```text
apps/ene-desktop-v2/
├── Cargo.toml        # winit, wgpu, pollster, bytemuck, glam, tracing, tracing-subscriber
└── src/
    ├── main.rs       # tracing-subscriber init + EventLoop::run_app
    ├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
    └── runtime.rs    # Runtime, WindowSlot, RectRenderer, ApplicationHandler, AcquireError
```

The full 7-module split planned in §5.2 (`runtime/{mod,input,surface,window_slot,loop,rect}.rs`, `gpu/{mod,depth,surface_format}.rs`, `platform/{mod,...}.rs`) was collapsed into the three files above. Reasons in the §22.3 "Files in this PR" block further down.

**Goal:** Get a winit + wgpu 27 window on Windows to (a) be transparent (DWM honors the swapchain's per-pixel alpha), (b) draw a single colored rectangle, and (c) toggle between transparent / opaque with `Space`, exit on `Escape`. No egui, no VRM, no AI bridge. Pure rendering smoke.

**Recipe that finally works (do all four together):**

1. **`wgpu::Dx12SwapchainKind::DxgiFromVisual`** — passed directly to `BackendOptions::dx12::presentation_system` in `apps/ene-desktop-v2/src/gpu.rs::backend_options` and forwarded to `wgpu::Instance::new`. This is the wgpu 27 DX12 backend option that creates the swapchain from the HWND's visual and is required for per-pixel alpha. The `WGPU_DX12_PRESENTATION_SYSTEM` env var has no effect on its own — wgpu 27 only consults it inside `Dx12SwapchainKind::from_env()`, which v2 does not call. Without `DxgiFromVisual`, the wgpu DX12 surface is created as `SurfaceTarget::WndHandle` and `Surface::get_capabilities` returns only `[CompositeAlphaMode::Opaque]` (see `wgpu-hal-27.0.4/src/dx12/adapter.rs:1006-1018`), which is the root cause of the persistent "opaque black" symptom in all earlier PR0 runs.

2. `WindowAttributesExtWindows::with_no_redirection_bitmap(true)` — set **in the `WindowAttributes` builder** in `apps/ene-desktop-v2/src/runtime.rs::window_attributes`, regardless of `transparent`. This adds `WS_EX_NOREDIRECTIONBITMAP` (0x00200000) to the HWND's exstyle at create time. **This is the piece that §22.1's `force_layered_window` approach was missing.** Setting only `WS_EX_LAYERED` (via `with_transparent(true)`) makes wgpu advertise `PreMultiplied` correctly, but DWM still composites the window through the redirection bitmap and shows the background as opaque black, because the swapchain's per-pixel alpha is not being read. Both styles must be present:

   ```text
   WS_EX_NOREDIRECTIONBITMAP = 0x00200000   (added by with_no_redirection_bitmap(true))
   ```

   > **WARNING — updated after PR0 closeout.** On winit 0.30.x, `with_transparent(true)` does **not** add `WS_EX_LAYERED` to the HWND by itself (only `WindowFlags::TRANSPARENT` is set, and that flag is consulted only in the legacy DWM blur-behind path which is skipped when `with_no_redirection_bitmap(true)` is also set). The only places winit 0.30 adds `WS_EX_LAYERED` to the exstyle are `with_ignore_cursor_events(true)`. **Do not try to fix the missing bit with `SetWindowLongPtrW(WS_EX_LAYERED)` from a post-create hook** — on the developer's PR0 machine, doing so on top of `WS_EX_NOREDIRECTIONBITMAP` caused DWM to revert the window to opaque-black composition (the legacy layered path apparently overrides the non-redirected path). The `with_no_redirection_bitmap` call in `window_attributes` is the only knob that should be touched.

3. `CompositeAlphaMode::PreMultiplied` in `SurfaceConfiguration` — **picked directly** by `gpu::pick_format_and_alpha` from the platform, **not** from `SurfaceCapabilities::alpha_modes`. The earlier implementation iterated the caps looking for `PreMultiplied` / `PostMultiplied` and fell back to `Opaque` when neither was listed; that was the root cause of the persistent "opaque black" symptom in earlier PR0 runs — the surface quietly degraded to `Opaque` instead of telling us it could not honour the request. The current implementation just returns `CompositeAlphaMode::PreMultiplied` on Windows / Linux and `CompositeAlphaMode::PostMultiplied` on macOS, matching `apps/tw-test` exactly. If the surface really does not support it, `Surface::configure` is called with an unsupported alpha mode and wgpu 27 will fail the next `get_current_texture`; the `Runtime::window_event` `AcquireError::Reconfigure` arm logs a clear `WARN` and reconfigures, so the failure is no longer silent. `WindowSlot::new` also emits an immediate `WARN` if the requested alpha mode is not in `SurfaceCapabilities::alpha_modes` — that is the single log line to look for to confirm the surface was misconfigured.

4. Clear to `(0, 0, 0, 0)` in transparent mode, `(0.2, 0.2, 0.2, 1.0)` in opaque mode — see `WindowSlot::render_frame`. The red quad is then drawn in the same pass with `LoadOp::Load` so the clear shows through.

**Cross-reference to the working `apps/tw-test` recipe:** the patched `bevy_winit` does both `with_transparent(true)` and `with_no_redirection_bitmap(true)` together at `patches/bevy_winit/src/winit_windows.rs:133-146`, and the `Window` resource sets `composite_alpha_mode: CompositeAlphaMode::PreMultiplied` directly with no fallback. PR0 reproduces both halves of that recipe in v2 without keeping the patch.

**Upstream wgpu UX issue (worth filing):** The combination of (a) `WGPU_DX12_PRESENTATION_SYSTEM` being listed in wgpu's own `README.md` (line ~169) as a supported env var, (b) the env var having *no effect* unless the user explicitly calls `Dx12SwapchainKind::from_env()` / `Dx12BackendOptions::from_env_or_default()`, and (c) the default `Dx12SwapchainKind::DxgiFromHwnd` silently giving `[Opaque]` only, is a footgun. A direct wgpu user who sets the env var and reads wgpu's `Surface::get_capabilities` sees a misleading "PreMultiplied not in caps" picture, and wgpu's own validation error (`Requested alpha mode PreMultiplied is not in the list of supported alpha modes: [Opaque]`) does not mention that `Dx12SwapchainKind::DxgiFromVisual` is the missing switch. Three reasonable fixes the wgpu maintainers could pick from:

1. **Auto-read the env var inside `wgpu::Instance::new` for DX12** so the env var "just works" the way every other `WGPU_*` var does, regardless of which code path creates the surface.
2. **Rename the env var** to something clearly opt-in (e.g. `WGPU_DX12_PRESENTATION_SYSTEM_OPT_IN`) and update the rustdoc on `Dx12SwapchainKind::from_env` to state that calling `from_env` is required.
3. **Improve the error message** in `Surface::configure` validation when the user asks for `PreMultiplied` on a `SurfaceTarget::WndHandle` surface: "hint: construct the instance with `Dx12SwapchainKind::DxgiFromVisual` to get a `SurfaceTarget::VisualFromWndHandle` that supports per-pixel alpha".

Filed against `gfx-rs/wgpu`.

**Diagnostic log on startup** (now emitted by `gpu::GpuContext::new` and `WindowSlot::new` so a future regression is one log line away; `tracing-subscriber` is initialised at the top of `main.rs` with `EnvFilter("info,wgpu_core=warn,wgpu_hal=warn,naga=warn")` by default — override with `RUST_LOG`):

```text
INFO  wgpu surface capabilities: formats=[Bgra8UnormSrgb], alpha_modes=[Opaque, PreMultiplied, PostMultiplied, Inherit, Auto]
INFO  SurfaceConfiguration picked: format=Bgra8UnormSrgb, alpha_mode=PreMultiplied
```

If the `caps.alpha_modes` list is `[Opaque]` only, the picker logs an explicit `WARN` and proceeds to call `Surface::configure` with `alpha_mode=PreMultiplied`. The first frame will then fail with `SurfaceError::Outdated` or `Lost`, the input loop will log "Surface acquire returned Outdated/Lost" repeatedly, and the user will see a clear `WARN` chain pointing at the misconfiguration. That is the signal to fall back to a manual `UpdateLayeredWindow` GDI path (out of scope for PR0) or to fix the wgpu/host environment. If `caps.alpha_modes` includes `PreMultiplied` but the visible result is still opaque black, the most likely cause is the missing `WS_EX_NOREDIRECTIONBITMAP` — confirm the workspace lock has winit 0.30 with the `WindowAttributesExtWindows` trait.

**Default start state:** `Runtime::new` initialises `transparent = false` (gray opaque window, decorations on, red rect). The user can press `Space` to try transparency. This is a UX safety net: on any environment where transparency does not work, the window is still visibly correct (gray + red rect) so the developer can confirm the rendering pipeline is alive.

**Keyboard / lifecycle (PR0 scope, see `Runtime::window_event`):**
- `Space` — toggle `transparent`. Calls `Window::set_decorations(!transparent)`. The clear color in `WindowSlot::render_frame` switches accordingly.
- `Escape` or close button — `EventLoop::exit()`.
- `Resized` / `ScaleFactorChanged` — `WindowSlot::reconfigure`, then `request_redraw`.
- `RedrawRequested` — `WindowSlot::render_frame`. On `SurfaceError::Outdated | Lost` we reconfigure and request another redraw.

**Files in this PR (per `git diff --stat`):**
- `apps/ene-desktop-v2/Cargo.toml` — slimmed to `winit`, `wgpu`, `pollster`, `bytemuck`, `glam`, `tracing`, `tracing-subscriber` (env-filter + fmt). No `raw-window-handle`, no `windows-sys`, no `ene-core`, no `tokio`, no `egui`, no `tray-icon`.
- **Layout: 3 files, ~330 lines total.** `src/main.rs` (tracing init + event loop), `src/gpu.rs` (`GpuContext` + `pick_format_and_alpha` + DX12 backend options), `src/runtime.rs` (`Runtime` + `WindowSlot` + `RectRenderer` + `ApplicationHandler` impl + `AcquireError`).
- Deleted: `src/gpu/{mod,surface_format}.rs` (folded into `gpu.rs`), `src/platform/{mod,linux,windows}.rs` (HWND exstyle diagnostic log removed; Linux display-server log removed — both were nice-to-have, not required for the recipe), `src/runtime/{mod,input,surface,window_slot,rect}.rs` (folded into `runtime.rs`).
- `RectRenderer` simplified: no UBO, no bind group, no pipeline layout with bindings. Hardcoded NDC `[-0.5, 0.5]²` quad (6-vertex `TriangleList`), hardcoded color in WGSL, empty pipeline layout. The original 211-line renderer (vertex+UBO+bind group+pipeline layout+WGSL) is now ~80 lines.
- Input handling inlined as match arms in the `ApplicationHandler::window_event` impl. The 7-argument `input::route` function is gone; `toggle_transparency` logic lives in the `Space` arm.
- `main.rs` — installs the `tracing_subscriber::fmt` subscriber with the `EnvFilter` above before any other work, so the wgpu caps and surface format are logged on startup.

**Manual smoke (verified on this developer's machine):**
1. `cargo run -p ene-desktop-v2` — gray opaque window with title bar, red rect at the center.
2. Press `Space` — window becomes borderless, background becomes the actual desktop (whatever is behind the window), red rect stays.
3. Press `Space` again — returns to gray + title bar.
4. Press `Escape` — exits cleanly.
5. Resize — frame is rebuilt, the red rect stays at NDC origin.

**Known limitations (carried over to PR1+):**
- v2 only has one window slot. The settings window, tray, AI bridge, and VRM renderer are PR1+.
- The wgpu 27 surface format picker still falls back to `Opaque` if the host's DXGI does not advertise `PreMultiplied`. In that case `Space` still toggles `decorations` and the clear color, but DWM composites the background as opaque.


