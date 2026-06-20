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
| **PR0 — `ene-desktop-v2` scaffold + Windows transparency smoke** | §22.3 | **Shipped** | `apps/ene-desktop-v2/` (3 source files, ~840 lines: `main.rs`, `gpu.rs`, `runtime.rs`) replaces the planned 7-module split. Single transparent window, red-quad renderer, `Space` toggles transparency, `Escape` exits. Verified on the developer's Windows machine. **Note:** the line count grew after PR2 (a second `winit` window with full `egui` integration — `CentralPanel` with a heading, text input, and click counter — is inlined in `runtime.rs::UiWindow`) so the file is no longer the PR0-minimum scaffold. The transparency recipe (§22.3) is unchanged. |
| **PR1 step 1 — `ene-vrm` crate skeleton** | §4 PR1 / step 2 | **Shipped** | `crates/ene-vrm/{Cargo.toml, src/lib.rs}` created with an empty `pub fn version()` stub and a unit test. No `gltf` / `wgpu` / `winit` deps yet. |
| **PR1 — v2: tray + AI bridge + `AppState` + persistence + CLI** | §4 PR1 | **Shipped** | `apps/ene-desktop-v2/` now has 7 source files (~1.5k LoC): `main.rs`, `gpu.rs`, `runtime.rs`, `state.rs`, `events.rs`, `settings.rs`, `ai_bridge.rs`, `tray.rs`. The `EneHandle` actor (from `ene-core`) is wrapped in `AiBridge`, the system tray (`tray-icon` 0.24) is wired on both Windows (dedicated `GetMessageW` thread) and Linux (GTK pump inside `about_to_wait`), `CharacterSettings` is ported from the legacy Bevy `Resource` to a plain `Arc<parking_lot::RwLock<…>>` struct backed by `ConfigStore`, and `Cargo run -p ene-desktop-v2` accepts the same `args[1]=vrm` / `args[2]=vrma` overrides the legacy app does. The legacy `apps/ene-desktop` (Bevy 0.18) is **deliberately untouched** per the new "v2 grows to full parity, then rename" policy — see §0.1 and §4. |
| **PR2 — v2: full settings UI (3 pages) + hotkeys + per-character config** | §4 PR2 | **Shipped** | `apps/ene-desktop-v2/src/settings_ui/` is a 5-file subtree (`mod.rs`, `input.rs`, `widgets.rs`, `page_graphics.rs`, `page_character.rs`, `page_ai.rs`); `apps/ene-desktop-v2/src/character_state.rs` carries PR2 stubs for `AnimationControl` and `EmotionCommand` / `EmotionQueue`. The settings window hosts a 3-tab strip (Character / Graphics / AI) bound to the same `CharacterSettings` fields the legacy `apps/ene-desktop/src/settings_ui/` exposed. F1 toggles visibility globally; `WASD` and `Space` cycle character/motion/play-pause on the character window while the settings window is open on the Character page; `Escape` closes (and saves); the AI page's "Send" button calls `AiBridge::run` and clears the chat input; the runtime auto-pops the settings window on `AiStreamUpdate::PermissionRequired` / `UserInputRequired` and seeds the `QuestionDraft` per item. The six manual expression-test buttons push to `EmotionQueue` for PR4 to consume. Legacy `apps/ene-desktop` (Bevy 0.18) is untouched. |
| **PR3 — v2: orthographic 3D camera + `ene-vrm` static rendering (MToon + skinning)** | §4 PR3 | **Shipped (MVP)** | `crates/ene-vrm/src/{lib,error,camera,model,renderer,loader,shaders/mtoon_lite.wgsl}.rs` (7 files, ~700 LoC). GLB-only loader via `gltf 1.4` (gated `KHR_materials_unlit` + `extensions` + `utils` features); decodes the first mesh's first primitive (POSITION / NORMAL / TEXCOORD_0), the first base-color texture (embedded data-URI / BIN chunk, PNG via `image 0.25`), and the first skin's inverse-bind matrices (stored, not yet applied). Single orthographic camera, depth-tested PBR-lite WGSL shader (half-Lambert + base color + premultiplied alpha), depth texture matched to the surface size. The v2 character window now renders `assets/characters/Alicia/AliciaSolid.vrm` instead of the red-quad smoke. **Out of scope for the MVP (deferred to follow-up PRs):** full MToon (rim / matcap / outline / emission), real skinning math, multi-mesh / multi-material handling, non-GLB (`.gltf`) VRM files, animations, expressions, look-at, drag, spring bone. |
| **PR4 — v2: LookAt / cursor / expressions / drag-to-move** | §4 PR4 | **Shipped** (PR4.1 ModelUniform + culling; **PR4.2 LookAt cursor projection + body-tracking profile**; **PR4.3 drag-to-move**; **PR4.4 expressions**; **PR4.5 skinning (rest-pose palette)**; **PR4.6 quick-win hardening**; **PR4.7 humanoid bone registry**; **PR4.8 `VRMC_vrm.lookAt` parse + per-frame evaluator**; **PR4.9 expression override + isBinary**; **PR4.10 alpha-mode sort + two-pass rendering**; **PR4.11 `KHR_materials_unlit` separate pipeline** (issue #19); **PR4.12 `VRMC_node_constraint` parse + evaluator** (issue #15); **PR4.13 `VRMC_springBone` parse + verlet simulator** (issue #13); **PR4.14 `VRMC_vrm_animation` (VRMA) parse + playback engine** (issue #14); **PR4.15 full `VRMC_materials_mtoon` parse + per-material uniform + MToon shader pipeline** (issue #12); **PR4.16 per-joint bone rotation from the evaluator + skin-palette upload** — closes PR4) | `apps/ene-desktop-v2/src/look_at.rs` ports the legacy `update_cursor_look_target` (lines 435–478) + `body_tracking_for_strength` (lines 514–537) 1:1. The runtime records `WindowEvent::CursorMoved` on the character window into `Runtime::last_cursor_logical`; `RedrawRequested` advances the smoothed target via `CharacterRenderer::update_look_at(cursor_logical, viewport, character_position, model_scale, strength, dt_secs)`. The smoothed world target lives on `CharacterRenderer::look_at: LookAtState` and is exposed via `look_at_target()`. The body-tracking profile (`BodyTracking { head/neck/chest/spine weights, yaw/pitch max, smoothing, output_smoothing, reference_depth }`) is computed on demand via `CharacterRenderer::body_tracking(strength)`. **PR4.8** added `crates/ene-vrm/src/look_at.rs` (`LookAtType { Bone, Expression }`, `LookAtRangeMap { input_max_value, output_scale }` with 90→10 default, `LookAtRangeMapSet { horizontal_inner, horizontal_outer, vertical_down, vertical_up }`, `LookAtProperties { offset_from_head_bone: [f32;3], range_map, look_at_type }` with spec defaults `[0, 0.06, 0]` / 90→10 / `"bone"`, `LookAtEvaluator` + `LookAtDirection { yaw_degrees, pitch_degrees }` + `LookAtBoneDelta { delta: Quat }` / `LookAtBoneOutput { head, left_eye, right_eye }` (for `"bone"`-type models) + `LookAtExpressionOutput { look_up, look_down, look_left, look_right }` (for `"expression"`-type models)). The loader calls `look_at::load_look_at` and stores the result in `VrmModel::look_at: Option<LookAtProperties>`. `calc_yaw_pitch` mirrors `bevy_vrm1::calc_yaw_pitch` (lines 222–237) 1:1: `atan2(x, z)` for yaw (positive = model looks to its own left), `atan2(-y, xz)` for pitch (positive = looks down). The head world position now comes from the humanoid registry's `head` bone (`character_position + head.rest.translation * model_scale`) when the model has one, falling back to the 1 m Y-offset constant for legacy VRM 0.x. The `CharacterRenderer` runs the evaluator every frame: for `"expression"`-type models the per-expression weights are written into `VrmModel::expressions_mut()` (the path the legacy `bevy_vrm1` silently no-op'd); for `"bone"`-type models the per-bone `Quat` deltas are stashed in `CharacterRenderer::look_at_bone_output` for the next skin-palette pass. **PR4.16** finally wires that per-frame palette upload: `VrmModel::update_skin_palette(&VrmaFrame, Option<&LookAtBoneOutput>)` now takes the LookAt bone deltas and, after the VRMA step but before the hierarchy walk, overwrites `local_rotations[head/leftEye/rightEye]` with `rest_local_rotations[node] * delta`. The LookAt step wins over the VRMA on those three bones (matches the legacy `bevy_vrm1` head-tracking-overrides-active-motion rule). `CharacterRenderer::update_motion` forwards `self.look_at_bone_output.as_ref()` so the head and eyes track the cursor in real time. `expression`-type models pass `None` (their LookAt signal routes into morph weights via `apply_emotions` instead). Four new unit tests in `crates/ene-vrm/src/model.rs` pin the behaviour. **PR4.4 expressions** landed a new `crates/ene-vrm/src/expression.rs` (`ExpressionName`, `PrimitiveMorphs`, `ExpressionLayer`, `PrimitiveMorphMeta` uniform with 16 packed `vec4` weight slots = 64 max targets per primitive), extended the glTF loader with `resolve_expression_names` that walks `VRMC_vrm.expressions.{preset,custom}.<name>.morphTargetBinds[*]`, added bind group `(3)` (storage + uniform) to `shaders/mtoon_lite.wgsl` with a `if (target_count > 0u)` early-out, and wired `EmotionQueue` → `CharacterRenderer::apply_emotions` in the runtime. The settings UI's six manual buttons and `AppEvent::EmoteToken` AI tokens both feed the queue; the renderer drains due commands once per frame, pushes weights into `VrmModel::expressions_mut().set_expression(...)`, and fades the active emotion back to zero after its `hold_secs` elapses. **PR4.6 quick-win hardening** closed six open issues: `MeshVertex::joints` widened from `[u8; 4]` to `[u32; 4]` (Uint32x4 attribute) so 256+-joint humanoid models no longer alias finger / wrist skinning onto `skin_matrices[255]` (#5); `load_primitive_morph_targets` now warns and `.take(MAX_MORPH_TARGETS_PER_PRIMITIVE)` truncates over-cap inputs (#6); `ExpressionLayer::set_expression` / `apply_weights` reject unknown expression names and do **not** store them in the weight map (#7); `load_vrm` logs a warning when a model ships `JOINTS_0` data without a skin — the case the renderer used to silently fall back to identity (#8); the `head_world = character_position + (0, 1, 0)` magic number in `Runtime::RedrawRequested` was hoisted into `look_at::HEAD_OFFSET_Y` + `head_world_for(pivot)` (#9); and a `const _: ()` block now pins the host-side `morph_offsets` element to 16 bytes so a regression to `[f32; 3]` would fail the build instead of silently misaligning the WGSL `array<vec3<f32>>` stride (#17). The `MAX_MORPH_TARGETS_PER_PRIMITIVE` const is re-exported from the crate root. **PR4.7 humanoid bone registry** adds a new `crates/ene-vrm/src/humanoid.rs` module (~480 LoC, 7 unit tests) that parses `VRMC_vrm.humanoid.humanBones` and exposes a `HumanoidBoneRegistry` (bone-name → `{ glTF node, optional Skeleton joint, rest translation + rotation }`). The 55-bone set is the VRM 1.0 spec canonical list (Hips..RightToes) with lower-case canonical names; `canonicalize_bone_name` accepts mixed-case / snake_case / kebab-case / PascalCase from hand-edited files. Helper accessors (`head()`, `hips()`, `left_eye()`, `right_eye()`, `jaw()`) make the deferred consumers (#11 LookAt, #13 SpringBone, #14 VRMA, #15 NodeConstraint) trivial to wire. Unknown bone names are dropped with `tracing::warn!`. The registry is attached to `VrmModel` and re-exported from the crate root. **No per-frame consumers land in PR4.7** — this PR only builds the registry and captures the rest transforms. |
| **PR5 — v2: click-through (Win32 subclass + Wayland input region + X11 shape) + offscreen mask** | §4 PR5 | **PR5.2 shipped (Windows)**, PR5.3+ pending | PR5.2 (2026-06-20 redesign): winit's `Window::set_cursor_hittest` is the only OS-facing touchpoint. The global cursor position is polled each frame via the `device_query` crate (`device_query::DeviceState::new().get_mouse().coords`) and converted to window-local using `Window::outer_position()` + the scale factor. The hit test is a Rapier raycast against **per-bone colliders** (sphere / capsule / Y-capsule) sized from the actual mesh's per-vertex skinning weights: `CharacterRenderer::build_character_bone_specs` walks `model.humanoid` once and emits a `BoneShapeSpec` per humanoid bone — `BoneShape::Sphere` for head / jaw / eyes / shoulders / toes, `BoneShape::Capsule` along the parent → bone rest direction for upper / lower arms + legs + hands, `BoneShape::CapsuleY` for neck / spine / chest / upperchest / hips / foot. `VrmPrimitive` retains `Vec<MeshVertex>` (cloned at load time) and `collider::collect_weighted_world_positions_into` filters vertices whose dominant skinning weight is `< VERTEX_WEIGHT_THRESHOLD = 0.25` for the target bone, so a vertex whose dominant weight targets the *shoulder* is not allowed to inflate the *arm* capsule. `PhysicsWorld::add_character_bone_colliders` registers them as a single kinematic body parked at the world origin. Every `about_to_wait`, after `update_motion` has refreshed `model.nodes.world_positions` *and* the per-bone rest → current world rotation, the runtime calls `update_character_bone_positions` which pushes each new pose into the matching collider via `set_translation_wrt_parent` *and* `set_rotation_wrt_parent` (axis-angle via `Quat::to_axis_angle`) — so the colliders follow the animation (both position and orientation) without any per-frame GPU readback. `allows_input = !transparent || cursor_over || drag.is_dragging()`; the `is_dragging` branch keeps the window receiving input across the drag. PR5.3 (Wayland `wl_surface::set_input_region`), PR5.4 (X11 shape), PR5.5 (Linux offscreen mask + gizmo) are still pending. |
| **PR5.5 — rename `apps/ene-desktop-v2` → `apps/ene-desktop`, delete legacy Bevy sources** | §4 PR5.5 | **Not started** | Gated on PR1–PR5 being feature-complete. |
| **PR5.6 — raycast collider debug overlay (F3 + settings checkbox)** | §4 PR5.6 | **Shipped** | New `ene_vrm::DebugRenderer` (line-list, 3D depth-tested) renders a wireframe sphere per PR5.2 bone collider (cyan idle, yellow hit) and a 3-axis cross at the raycast hit point (red). Pipeline uses `Depth32Float`, `depth_compare = Less`, `depth_write = false`, premultiplied alpha blending, `LoadOp::Load` on both color and depth so the model silhouette correctly occludes the wires. The `AppState` now carries the latest `RaycastHit` (entity / toi / world-space point / collider handle) so the runtime can highlight the exact collider that was hit and draw the hit-point cross. The overlay is toggled by **F3** on the character window or by a new "Raycast Colliders (Debug)" checkbox on the Character settings page; the state is **not persisted** — the overlay is OFF on every launch. |
| **PR5.7 — bypass winit 0.30.13 multi-window `RedrawRequested` bug via direct-render in `about_to_wait`** | §4 PR5.7 | **Shipped** | The character window's render is now driven directly from `Runtime::about_to_wait` (via a new `render_char_frame` method), sidestepping the `Window::request_redraw` → `Event::WindowEvent::RedrawRequested` → render path entirely. The UI window already rendered this way for the same reason; this PR aligns the char window to that pattern. The winit 0.30.13 bug (`ControlFlow::WaitUntil(deadline)` + 2 windows → the first window ever created never receives `RedrawRequested`; verified by `apps/winit-waituntil-bug-repro`) becomes irrelevant because we no longer depend on the event. With the char render decoupled from `RedrawRequested` delivery, the `WaitUntil(Instant + frame_interval)` frame pacer is restored: `WaitUntil(deadline)` for `15/30/60/120 FPS`, `Poll` for `0` ("Unlimited"). The deadline is anchored to `last_frame_instant` (updated at the top of `render_char_frame`) with a `now()` fallback for the cold start, so the `dt_secs` used by `update_motion` lines up with the chosen rate. The `resumed` `set_control_flow(Poll)` stays as a one-shot cold-start default. The `RedrawRequested` arm of `handle_char_window_event` is a no-op (kept for clarity / future re-enable). A `Runtime::char_surface_fatal` flag is set on `AcquireError::Fatal` and the event loop exits at the tail of the next `about_to_wait` (instead of the previous inline `event_loop.exit()` in the now-removed `RedrawRequested` arm). `apps/winit-waituntil-bug-repro` ships as a permanent regression test + upstream-report artifact (see `apps/winit-waituntil-bug-repro/WINIT-ISSUE.md`). |

### 0.1 How the two desktop apps coexist

Both binaries build side-by-side throughout the entire migration:

- **`apps/ene-desktop`** — legacy Bevy 0.18 build. Still the user-facing desktop app. Still depends on `bevy`, `bevy_egui`, `bevy_vrm1`, `tray-icon`, `gtk` (Linux), and `wayland-client` (Linux). The local `patches/bevy_winit` patch has been removed; the legacy build now uses upstream `bevy_winit` 0.18 from crates.io. **The legacy binary is not being modified by this migration at all** — it stays as-is until the rename step.
- **`apps/ene-desktop-v2`** — new crate, lives next to the legacy one. Started as a `winit` + `wgpu` 29 + `egui` 0.34 transparent-window smoke (PR0, §22.3). PR1 added a tokio-runtime-driven `AppState`, an `EneHandle` wrapper, a system tray, and a `CharacterSettings` port. **Cargo `run -p ene-desktop-v2`** to launch.

The original plan called for "PR1 is the deletion step" — strip Bevy from `apps/ene-desktop` and move the v2 sources into it. That plan is **superseded by the new policy below** so we never have a half-broken intermediate state where a feature works in one binary and not the other.

#### New policy (PR1 onwards)

> **v2 grows incrementally through PR1–PR5+ until it is a feature-complete replacement for the legacy `apps/ene-desktop` (Bevy 0.18). The legacy crate keeps building throughout the migration. The rename `apps/ene-desktop-v2/` → `apps/ene-desktop/` and the deletion of the legacy Bevy sources happen in a single commit at the end of PR5 (gated by §0's PR1–PR5 all being "Shipped").**

The full PR roadmap is in §4. Treat the two crates as **parallel codebases** until PR5.5.

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
- It also lets us switch shader backends (wgpu 29) without touching the desktop binary.

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
      wgpu 29, winit 0.30, egui 0.34 (egui-wgpu, egui-winit)
      gltf 1.4, glam 0.33, encase 0.12, bytemuck, pollster
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

### PR1 — v2: tray + AI bridge + `AppState` + persistence + CLI

> **Status:** **Shipped** (commit `9b4a2d4` pending). The v2 crate now boots a tokio runtime, owns a tokio-driven `EneHandle` actor, a system tray (`tray-icon` 0.24), and the `CharacterSettings` schema (ported from the legacy Bevy `Resource` to a plain struct). The legacy `apps/ene-desktop` is unchanged.

**Objective:** Wire the non-rendering half of the desktop app (AI bridge, tray, settings persistence) into v2, and bring v2 up to the same non-rendering feature level the legacy Bevy app has today, but without Bevy. Visual rendering is still PR0's red-quad smoke on the character window and the PR0 + PR2 egui demo on the UI window — the full VRM, LookAt, drag, and click-through features arrive in PR3–PR5.

**Scope (shipped)**

1. **Workspace `Cargo.toml`**: add `parking_lot = "0.12"` (for non-poisoning `RwLock`), `png = "0.18"` (tray icon decode), `tray-icon = { version = "0.24", default-features = false }` (tray). All other rendering-stack deps from PR0 stay.
2. **`apps/ene-desktop-v2/Cargo.toml`**: add `ene-core`, `ene-config`, `ene-tool-proto` (the non-rendering half of the legacy app's dep graph), `tokio` (workspace), `parking_lot`, `png`, `tray-icon`, `ctor`, `serde`, `serde_json`, `thiserror`. Linux target adds `gtk = "0.18.2"` and enables the `gtk` feature on `tray-icon`.
3. **`apps/ene-desktop-v2/src/events.rs` (new)**: `AppEvent` enum (Tray / Ai / EmoteToken / Quit variants) and the `AiStreamUpdate` enum (9 flattened variants of `ene_core::EneEvent`). The bus is a `tokio::sync::mpsc::unbounded_channel`. The `Sender` is cloned into the tray thread and into the AI bridge's pump task; the `Receiver` lives in `Runtime` and is drained in `about_to_wait`.
4. **`apps/ene-desktop-v2/src/settings.rs` (new)**: port of legacy `apps/ene-desktop/src/app_config.rs` to a plain struct (no Bevy `Resource`). `CharacterSettings { assets_dir, characters, graphics, character_state, ui, ai, store: Arc<parking_lot::RwLock<ConfigStore>> }`. Includes `GraphicsSection` (declared via `ene_config::define_config!(settings, "desktop", DesktopSection { graphics })`), `ShadowQuality` / `AntialiasingMode` enums via `define_label_enum!`, `CharacterEntry`, `CharacterState`, `UiState`, `AiConfig`, `discover()`, `current_*()`, `select_character()`, `save()` / `mark_dirty()` / `flush_if_dirty()` / `sync_to_store()` / `load_from_file()`. Also `read_cli_paths()` (the `args[1]=vrm` / `args[2]=vrma` parser) and the cycle helpers. The legacy file stays put; the port is structural-only.
5. **`apps/ene-desktop-v2/src/ai_bridge.rs` (new)**: `AiBridge` wraps `EneHandle`. Constructor spawns (a) a `pump_events` tokio task that loops `try_recv` on `EneHandle::subscribe()` and pushes `AiStreamUpdate` into an inbox, and (b) a `bootstrap` tokio task that calls `handle.load_config()` then `handle.load_character()`. Exposes `run(&str)`, `cancel()`, `drain()` (for the main thread), `take_inbox()`, and a `latest_response()` reader. `<|emo:NAME|>` tokens are pulled out of `SpecialToken` events and forwarded as `AppEvent::EmoteToken`; the actual `EmotionQueue` (which drives VRM morph weights) lands in PR4.
6. **`apps/ene-desktop-v2/src/tray.rs` (new)**: `TrayHandle`. On Windows, spawns a dedicated thread that runs `TrayIconBuilder` + a hand-rolled `GetMessageW` / `TranslateMessage` / `DispatchMessageW` loop (the legacy code's recipe), `mem::forget`s the icon so the thread keeps it alive. On Linux, builds the icon on the main thread and a separate thread polls `TrayIconEvent::receiver()` and `MenuEvent::receiver()`. A `tick_gtk()` method is called from `Runtime::about_to_wait` to pump `gtk::main_iteration_do(false)`. The menu is "Settings" and "Quit" (matching the legacy). PNG decode accepts `Rgba` / `Rgb` / `Grayscale` / `GrayscaleAlpha`, falls back to a 32×32 opaque blue square if `assets/icon.png` is missing.
7. **`apps/ene-desktop-v2/src/state.rs` (new)**: `AppState { gpu, settings, ai: Arc<AiBridge>, tray: Option<TrayHandle>, event_rx }`. `with_channel(gpu, settings, &handle)` builds a paired `(AppState, AppEventSender)`. The `Sender` is handed to `AiBridge::new` and to `TrayHandle::new`. Exposes `init_tray`, `ai_run`, `save`, `request_quit`, `resolve_paths` (helper for `main`).
8. **`apps/ene-desktop-v2/src/main.rs` (rewrite)**: tracing init → `tokio::runtime::Builder::new_multi_thread().enable_all().build()` + `runtime.enter()` guard → `resolve_paths()` → `gpu::GpuContext::new` → `CharacterSettings::discover` → `AppState::with_channel` → `EventLoop::new` → `Runtime::new(state, event_tx).run_app`.
9. **`apps/ene-desktop-v2/src/runtime.rs` (refactor)**: `Runtime { state, event_tx, transparent, char_window, ui_window, rect }`. `ApplicationHandler`:
   - `resumed` — `init_tray` (which also dispatches the GTK pump start on Linux) → create the `CharacterWindow` and the `UiWindow` (PR0 + PR2 code, unchanged).
   - `window_event` — dispatches by `WindowId` (char vs ui), handles `Space` / `Escape` / `close` / `Resized` / `ScaleFactorChanged` / `RedrawRequested` / `KeyboardInput`.
   - `about_to_wait` — drain `event_rx`: on `Tray::OpenSettings` set `settings.ui.settings_window_visible = true`; on `Ai(_)` push the update to `AiBridge::take_inbox` (the PR2 egui page will render from it); on `Quit` save and `event_loop.exit()`; on `EmoteToken(_)` stash for PR4; always call `settings.flush_if_dirty(Some(&char_name))`; on Linux call `state.tray.tick_gtk()`; redraw both windows.
10. **Verification (PR1 closeout):**
    - `cargo build -p ene-desktop-v2` succeeds.
    - `cargo clippy -p ene-desktop-v2 -- -D warnings` clean. Most of the new public API (`AiBridge::run`, `AppEvent::Quit`, `AiStreamUpdate` variants, several `CharacterSettings` helpers) is `#[allow(dead_code)]`-annotated because the consumers land in PR2; the annotations are scoped and will be removed as PR2–PR5 land.
    - `cargo clippy --workspace --exclude ene-desktop -- -D warnings` clean. Legacy `apps/ene-desktop` clippy errors are **pre-existing** and out of scope for this migration (the live `render_settings_window` in the legacy code is a "test" stub, so `page_ai_page` / `page_character_page` / `page_graphics_page` are reported as dead — left for the legacy maintainers).
    - `cargo test --workspace`: 198 passed, 6 ignored. No new tests added by PR1 (all the action is plumbing; the first real behavior tests land with the settings UI in PR2).
    - Manual smoke on Windows: `cargo run -p ene-desktop-v2` shows the system tray icon "ene", left-click opens the UI window (PR0 + PR2 demo), "Settings" tray menu sets `ui.settings_window_visible` (currently a no-op visual; PR2 wires the page), "Quit" exits cleanly. The character window still shows the red-quad smoke from PR0. The legacy `cargo run -p ene-desktop` is unchanged and still launches the Bevy 0.18 build.

**Files touched / created (PR1)**

- `Cargo.toml` (workspace) — add `parking_lot`, `png`, `tray-icon`.
- `apps/ene-desktop-v2/Cargo.toml` — full rewrite: new deps + Linux-target section.
- `apps/ene-desktop-v2/src/main.rs` — rewrite (tokio runtime + `AppState::with_channel`).
- `apps/ene-desktop-v2/src/runtime.rs` — refactor (ApplicationHandler now owns `state` + drains `event_rx` in `about_to_wait`).
- `apps/ene-desktop-v2/src/events.rs` — **new**.
- `apps/ene-desktop-v2/src/settings.rs` — **new**.
- `apps/ene-desktop-v2/src/ai_bridge.rs` — **new**.
- `apps/ene-desktop-v2/src/tray.rs` — **new**.
- `apps/ene-desktop-v2/src/state.rs` — **new**.
- No changes to `apps/ene-desktop/` (legacy) — it stays as-is. The rename is deferred to PR5.5.

### PR2 — v2: full settings UI (3 pages) + hotkeys + per-character config

> **Status:** **Shipped.** `apps/ene-desktop-v2/src/settings_ui/` is a 5-file subtree; `apps/ene-desktop-v2/src/character_state.rs` carries PR2 stubs for `AnimationControl` and `EmotionCommand` / `EmotionQueue`. The 460×620 settings window hosts a 3-tab strip (Character / Graphics / AI) bound to the same `CharacterSettings` fields the legacy `apps/ene-desktop/src/settings_ui/` exposed. F1 toggles visibility globally; `WASD` and `Space` cycle character / motion / play-pause on the character window while the settings window is open on the Character page; `Escape` closes (and saves); the AI page's "Send" button calls `AiBridge::run` and clears the chat input; the runtime auto-pops the settings window on `AiStreamUpdate::PermissionRequired` / `UserInputRequired` and seeds the `QuestionDraft` per item. The six manual expression-test buttons push to `EmotionQueue` for PR4 to consume.

**Objective:** Port the legacy `apps/ene-desktop/src/settings_ui/` (3 pages: AI / Character / Graphics, 6 hardcoded test buttons for expressions, F1/Esc/close lifecycle, F1 global hotkey, WASD/space hotkeys on the Character page when egui is unfocused, settings auto-popup on `PermissionRequired` / `UserInputRequired`, per-character `CharacterConfig` round-trip) into v2 as tab pages inside the existing `UiWindow`.

**Scope (shipped)**

1. ✅ `UiWindow::render_frame` body rewritten; the PR0+PR2 egui demo is gone, replaced by a `CentralPanel` that hosts the new `SettingsUi` tab strip (`Character` / `Graphics` / `Ai`) and dispatches to per-page render functions.
2. ✅ `apps/ene-desktop-v2/src/settings_ui/mod.rs` — new module. Owns `PageKind`, `SettingsUi`, the per-frame `render(&mut self, ui, &mut CharacterSettings, &Arc<AiBridge>)` entry point, and `apply_egui_visuals` (the dark theme from legacy `mod.rs:642-654`).
3. ✅ `apps/ene-desktop-v2/src/settings_ui/page_ai.rs` — provider / model / base URL / API-key source (inline vs env) / API-key env-var / inline key (password field) / embedding provider (cloud / local) / embedding model / embedding dimensions / "Enable Long-term memory" checkbox / chat input + Send / latest-response scroll area. "Send" and Enter both call `ai.run(&input)` directly (legacy used a `MessageWriter<EneRequestEvent>`; v2's `AiBridge` is `Arc`d and callable).
4. ✅ `apps/ene-desktop-v2/src/settings_ui/page_character.rs` — character cycle / motion cycle / animation play-pause / look-at strength / model scale / position X / Y / Z / 6 manual expression buttons (push `EmotionCommand` to `SettingsUi::emotion_queue`; PR4's renderer will pop). Linux-only debug-overlay toggle and mask-downsample row are in place (gated on `cfg(target_os = "linux")`).
5. ✅ `apps/ene-desktop-v2/src/settings_ui/page_graphics.rs` — target FPS (cycle), shadow quality (cycle), antialiasing mode (cycle). The fields land in `CharacterSettings::graphics` (same as legacy) and are forwarded to `ConfigStore` by the PR1 `sync_to_store`. PR3 will read them and wire the actual shadow-map size / AA mode on the wgpu pipeline.
6. ✅ `apps/ene-desktop-v2/src/settings_ui/widgets.rs` — `SettingsAction` enum (40+ variants) + `apply_action` dispatcher (port of legacy `settings_ui/widgets.rs`). Cycle / toggle / numeric row helpers live inline in the per-page modules.
7. ✅ `apps/ene-desktop-v2/src/settings_ui/input.rs` — `SettingsInputState` (text buffers for each `TextEdit`) and `sync_from_settings` (called when the window transitions hidden → visible).
8. ✅ Lifecycle: on F1-toggle-off / Esc / `WindowCloseRequested` on the UI window, `state.save()` is called. The PR1 `about_to_wait` already calls `state.settings.flush_if_dirty()` every frame.
9. ✅ Hotkeys: `F1` toggles globally (handled in the character window's `KeyboardInput` arm, so it fires whether the UI window is open or hidden). `W` / `A` / `S` / `D` cycle character / motion on the character window when the settings window is open and the current page is `Character`. `Space` on the character window toggles transparency (PR0 smoke); the legacy code had the same dual binding.
10. ✅ Auto-popup: the runtime observes `AiStreamUpdate::PermissionRequired` and `UserInputRequired` in `about_to_wait`, populates `UiState::pending_permission` / `pending_user_input` / `user_input_drafts`, and sets `settings_window_visible = true`. A future PR will render the dialog; the data path is wired.
11. ✅ `AiStreamUpdate::TextDelta` deltas are appended to `UiState::ai_latest_response` in `about_to_wait`, which the AI page's "Latest Response" scroll area already reads.

**Verification**

- `cargo build -p ene-desktop-v2` clean.
- `cargo clippy -p ene-desktop-v2 -- -D warnings` clean.
- `cargo clippy --workspace --exclude ene-desktop -- -D warnings` clean.
- `cargo test --workspace` — 198 passed, 6 ignored (37 suites). No new tests yet; the unit-test for `CharacterSettings::discover` / `select_character` round-trip is on the PR3 backlog (it needs the vrm/character files under `assets/`).
- Legacy `apps/ene-desktop` (Bevy 0.18) **still builds** (`cargo build -p ene-desktop` returns 0 errors; the 31 warnings are pre-existing from the dead `render_settings_window` test stub and are out of scope for this PR).

**Files touched / created (PR2, shipped)**

- **New** — `apps/ene-desktop-v2/src/character_state.rs` (38 lines; PR2 stubs for `AnimationControl`, `EmotionCommand`, `EmotionQueue`).
- **New** — `apps/ene-desktop-v2/src/settings_ui/{mod,input,page_ai,page_character,page_graphics,widgets}.rs` (5 files, ~750 lines total).
- **Modified** — `apps/ene-desktop-v2/src/main.rs` (`mod character_state;` and `mod settings_ui;`).
- **Modified** — `apps/ene-desktop-v2/src/runtime.rs` — F1 / WASD / Space arms in the character-window `KeyboardInput` handler; the UI-window `KeyboardInput` handles Esc; `about_to_wait` gained the auto-popup logic; `UiWindow` now holds a `SettingsUi` and calls `settings_ui.render(ui, &mut CharacterSettings, &Arc<AiBridge>)`; helper methods `show_settings_window` / `hide_settings_window` on `Runtime`.
- **Modified** — `apps/ene-desktop-v2/src/settings.rs` — added `UiState::pending_permission` / `pending_user_input` / `user_input_drafts`; new `PendingPermission` / `PendingUserInput` / `QuestionDraft` types.

**Known limitations (deferred to later PRs)**

- The "pending permission / question" dialogs are now rendered (A.5). A `PendingPermission` shows a centered `egui::Window` with the action / target / description and three buttons (Yes → `PermissionDecision::AllowOnce`, No → `Deny`, Always → `AllowSession`); a `PendingUserInput` shows a multi-row form with one collapsing section per `QuestionItem` — predefined options become `selectable_label` buttons, free-text fields become a `TextEdit`, and a Skip checkbox packs a `MultiAnswer::Skip`. Submit packs a `UserInputResponse::Multi(Vec<MultiAnswer>)`; Cancel packs `UserInputResponse::Cancel`. Both dialogs also forward a deny / cancel on window-close so a dismissed dialog doesn't stall the actor's oneshot. The `#[allow(dead_code)]` and `#[expect(dead_code)]` markers on `UiState::pending_*` and the three dialog types are removed.
- The "Send" button (and the chat input) now re-enable / disable based on `AiBridge::is_processing()` (A.4). The flag is a lock-free `Arc<AtomicBool>` set to `true` on `AiBridge::run` and cleared by the `pump_events` task on `EneEvent::Done` / `EneEvent::Failed`. The hint text switches to `"waiting for AI…"` while the flag is set so the user can see the in-flight state.
- Numeric row text fields (`LookAt Strength` etc.) now re-parse the buffer on Enter / focus loss (PR2.1); the +/- buttons are still the primary input but typing a value and pressing Enter (or tabbing out) now commits. A failed `f32::parse` reverts the buffer to the live setting, so out-of-range / non-numeric input cannot poison the model.

### PR3 — v2: orthographic 3D camera + `ene-vrm` static rendering (MToon + skinning)

> **Status:** **Shipped (MVP).** `crates/ene-vrm` is now a real loader + renderer. The v2 character window renders the bundled `AliciaSolid.vrm` (6 MB GLB) instead of the red-quad smoke. The full MToon material model (rim / matcap / outline / emission) and the joint-math skinning ship as follow-up PRs alongside animations, expressions, look-at, drag, and spring bone.

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

### PR4 — v2: LookAt / cursor / expressions / drag-to-move

> **Status:** **Shipped** (2026-06-18, commit pending). **PR4.1 (ModelUniform + culling)**, **PR4.2 (LookAt cursor projection + body-tracking profile)**, **PR4.3 (drag-to-move)**, **PR4.4 (expressions / morph targets)**, **PR4.5 (skinning — rest-pose palette)**, **PR4.6 (quick-win hardening)**, **PR4.7 (humanoid bone registry)**, **PR4.8 (`VRMC_vrm.lookAt` parse + per-frame evaluator)**, **PR4.9 (expression override + isBinary)**, **PR4.10 (alpha-mode sort + two-pass rendering)**, **PR4.11 (`KHR_materials_unlit` separate pipeline)** (issue #19), **PR4.12 (`VRMC_node_constraint` parse + evaluator)** (issue #15), **PR4.13 (`VRMC_springBone` parse + verlet simulator)** (issue #13), **PR4.14 (`VRMC_vrm_animation` (VRMA) parse + playback engine)** (issue #14), **PR4.15 (full `VRMC_materials_mtoon` parse + per-material uniform + MToon shader pipeline)** (issue #12), and **PR4.16 (per-joint bone rotation from the evaluator + skin-palette upload)** — closes PR4. The PR4 step list below is unchanged from the legacy plan (Expressions, LookAt, BodyTracking all port as before), but the consumers move to `apps/ene-desktop-v2::character` (new) / `runtime` (existing), and a new drag-to-move step (which used to live in the legacy `character_drag/mod.rs` plugin) gets its first cut here — the full click-through story (Win32 `WM_NCHITTEST`, Wayland `wl_surface::set_input_region`, X11 shape extension, offscreen mask capture) is PR5.

**Objective:** Make the character react to the cursor and to AI-emitted emotions; let the user click-and-drag the character to reposition it on the desktop.

**PR4.4 progress (shipped this PR)**

- New `crates/ene-vrm/src/expression.rs`:
  - `ExpressionName` newtype (case preserved, lower-case canonical per VRM 1.0).
  - `PrimitiveId`, `MorphTarget { name, position_offsets: Vec<[f32; 3]> }`, `PrimitiveMorphs { primitive_id, targets, name_to_slot: BTreeMap<ExpressionName, u32>, target_count, vertex_count }` — one per primitive that defines morph targets.
  - `ExpressionLayer { per_primitive: Vec<Option<PrimitiveMorphs>>, weights: BTreeMap<ExpressionName, f32> }` attached to `VrmModel`.
  - `PrimitiveMorphMeta` uniform (`#[repr(C)] Pod`): `vertex_count: u32`, `target_count: u32`, two `u32` pads, then `[[f32; 4]; 16]` packed weights (= 64 slots max per primitive). Mirrors the WGSL `MorphMeta` struct byte-for-byte.
  - 8 unit tests covering dedup across primitives, slot lookup, weight clamping, `apply_weights` overwrite, default-state.
- Loader (`crates/ene-vrm/src/loader.rs`):
  - `load_primitive_morph_targets` reads `primitive.reader(...).read_morph_targets()` POSITION displacements, normalises them by the loader's `(center, scale)`, pads to the primitive's vertex count, and pairs each entry with a name.
  - `resolve_expression_names(gltf)` walks `Document::extensions()["VRMC_vrm"]["expressions"].{preset,custom}.<name>.morphTargetBinds[*]`, resolves each `{node, index}` to `(mesh_idx, prim_idx, morph_target_index)` via `Node::mesh().index()`, and binds the real name (e.g. `happy`, `sad`) to **all** primitives of that mesh (the spec says "all primitives must share the same morphTarget"). Falls back to `morph_target_<i>` for targets not referenced by the extension.
- Renderer (`crates/ene-vrm/src/renderer.rs`):
  - New bind group layout `(3)` — `storage<read>` for `morph_offsets: array<vec3<f32>>` (length `target_count * vertex_count`) and `uniform` for `morph_meta` with `min_binding_size = PrimitiveMorphMeta::SIZE`.
  - One `MorphGpu { offsets_buf, meta_buf, bind_group }` per morph-bearing primitive; a single `DummyMorphGpu` shared by every primitive without morphs (the shader's `target_count == 0u` early-out skips the lookup).
  - `upload_morph_meta` walks `prim_morphs.targets.iter().enumerate()` (local slot index), looks up the global weight in `model.expressions().weights`, and packs it into `meta.weights[slot/4][slot%4]`. Local-index, not global-flattened — the shader's `weights[t/4][t%4]` lookup matches the storage buffer row that was filled by the same iteration order.
- Shader (`crates/ene-vrm/src/shaders/mtoon_lite.wgsl`):
  - `struct MorphMeta { vertex_count, target_count, _pad0, _pad1, weights: array<vec4<f32>, 16> }`.
  - `@group(3) @binding(0) var<storage, read> morph_offsets: array<vec3<f32>>`, `@group(3) @binding(1) var<uniform> morph_meta: MorphMeta`.
  - `vs_main` takes `@builtin(vertex_index) vidx: u32`, accumulates `morph_offsets[t * vertex_count + vidx] * weights[t/4][t%4]` into a `morph_delta: vec3<f32>`, and adds it to `world_pos` before `view_proj * world_pos`.
- Runtime wiring (`apps/ene-desktop-v2`):
  - `EmotionCommand` now carries `weight: f32` (default `1.0` for both the AI bridge and the manual buttons). `EmotionQueue::drain_due(now_secs)` separates due commands from future-scheduled ones (lipsync placeholder).
  - `ActiveEmotion { name, weight, hold_until_secs }` is the renderer's "currently shown" emotion.
  - `CharacterRenderer::apply_emotions(&mut EmotionQueue, now_secs)` is called once per frame from `Runtime::about_to_wait` (right after the `AppEvent::EmoteToken` drain loop, before redraw). Drains due commands, calls `VrmModel::expressions_mut().set_expression(&ExpressionName::from(name.as_str()), weight)`, and fades the active emotion multiplicatively by `FADE_RATE = 0.9` per frame after `hold_secs` elapses (drops the slot to `None` once weight falls below `FADE_FLOOR = 0.01`).
  - **Weight-clearing invariant:** the emotion-to-weight pipeline is a *replace*, not a *merge*. Switching from `happy` → `neutral` (which is not a morph target on the bundled Alicia model) must zero out the previous `happy` weight or the shader keeps squinting the eyes. A pure helper `transition_emotions(drained, current, now_secs, fade_rate, fade_floor) -> (Option<ActiveEmotion>, Vec<(String, f32)>)` lives in `character_state::transition_emotions`; when a new command arrives with a different name, it emits `(prev.name, 0.0)` *before* `(new.name, weight)` so the renderer's `set_expression` applies the clear first. This mirrors the legacy Bevy app's `SetExpressions` resource, which implicitly dropped missing names every frame.
  - 4 new unit tests in `character_state::tests` covering drain ordering (due / future / empty / preserve-remaining-order), plus 4 transition-emotion tests including a regression test (`transition_emotions_clears_previous_when_switching`) that locks the invariant above.
  - **Deferred:** normal / tangent morph displacements, multi-target blend-shape graphs (e.g. `blink_l + blink_r → blink`), look-at `expression` mode (writes `lookLeft/Right/Up/Down`), full MToon material model.

**PR4.3 progress (shipped this PR)**

- New `apps/ene-desktop-v2/src/character/` folder (replaces the old `character.rs`):
  - `mod.rs` — `CharacterRenderer` gains `pub drag: CharacterDragState` and a `aabb_world(&ModelUniform)` accessor.
  - `drag.rs` — 1:1 port of `apps/ene-desktop/src/character_drag/mod.rs` adapted to the v2 stack.
- State machine (`apps/ene-desktop-v2/src/character/drag.rs`):
  - `CharacterDragState { last_cursor_world_pos: Option<Vec2> }` + `is_dragging()` (the latter is `#[allow(dead_code)]` until PR5.1 wires it into the click-through `allows_input`).
  - `enum DragButtonEvent { Pressed, Released }` + `enum DragAction { None, Started, Ended }` (the `tick` helper returns `Option<Vec3>` directly — the per-frame delta — so `DragAction` stays press/release-only).
  - `on_press_or_release(state, event, cursor_world_2d, cursor_over_character) -> DragAction`: pressed-over-character starts a drag (stores the world cursor), released ends it. Released-while-not-dragging is a no-op.
  - `tick(state, cursor_world_2d) -> Option<Vec3>`: per-frame delta `(new - last).extend(0.0)`, identical math to the legacy `update_drag_state`.
- Math helpers (all 1:1 with the legacy):
  - `aabb_world_corners` (8-corner transform) and `transformed_aabb_bounds` (world AABB).
  - `ray_intersects_aabb` (slab test, eps=1e-6, axis-by-axis closure, identical).
  - `cursor_logical_to_world_2d` (NDC → view-z=0 plane → world 2D) — mirrors the `view_pos = Vec3::new(ndc.x * half_w, ndc.y * half_h, 0)` pattern from `look_at::compute_world_target`. For ortho the absolute world point is the camera's eye; the drag system only cares about the *delta* between two samples, matching Bevy's `Camera::viewport_to_world_2d` semantics.
  - `cursor_over_character` (per-frame hit test: cursor-ray vs. transformed world AABB).
- Runtime wiring (`apps/ene-desktop-v2/src/runtime.rs`):
  - New helpers `cursor_world_2d_for_char_window(cw, position)` and `cursor_over_char_window(cw, character, settings, position)` that wrap the projection + hit test for the winit `PhysicalPosition` input.
  - `WindowEvent::CursorMoved` now also calls `character::drag::tick(&mut character.drag, cursor_world_2d)` and, on a non-`None` delta, integrates it into `settings.character_state.character_position` (mut-borrow split with the surrounding handler so `cw` and `character` are borrowed independently).
  - `WindowEvent::MouseInput { state, button: Left }` calls `character::drag::on_press_or_release` with the right `DragButtonEvent`; on `DragAction::Ended` the runtime calls `settings.mark_dirty()`.
  - New import: `winit::event::MouseButton` (alongside the existing `ElementState`).
- 12 new unit tests in `character::drag::tests` cover: AABB transform (translation + scale), identity bounds, ray hits + misses (axis-aligned), press-over-character starts a drag, press-outside-character is no-op, press-without-cursor-pos is no-op, release-while-dragging ends, release-while-idle is no-op, tick-when-idle is `None`, tick-when-unchanged is `None`, tick-when-moved returns delta + advances origin, tick-without-cursor keeps state, ortho center projects to the camera's eye, world delta is proportional to the cursor pixel delta, degenerate viewport is `None`.
- **Out of scope for PR4.3:** click-through / passthrough. The runtime does **not** override the winit hit-test, so the entire character window is still clickable. PR5.1 (Windows: `SetWindowSubclass` + `WM_NCHITTEST` + `WS_EX_TRANSPARENT`) and PR5.2 (Wayland: `wl_surface::set_input_region`) carry that work. The drag state's `is_dragging()` accessor is reserved for the `allows_input = cursor_over_character || drag_state.is_dragging()` predicate those PRs will use.

**PR4.16 progress (shipped this PR)** — closes PR4

- `VrmModel::update_skin_palette` (`crates/ene-vrm/src/model.rs`) gains a second parameter `look_at: Option<&LookAtBoneOutput>`. For `"bone"`-type models the runtime feeds `CharacterRenderer::look_at_bone_output()` (the smoothed output of `LookAtEvaluator`); for `"expression"`-type models and for VRMs that don't define `lookAt` the call site passes `None` and the existing rest-pose math is unchanged. The signature is backward-compatible — the test helper `model_palette(model, frame)` simply passes `None` for the new argument.
- Composition is a new **step 2.5** between VRMA bone rotations and the hierarchy walk. For each of `head`, `leftEye`, `rightEye` the function looks up the humanoid bone (gracefully skipping models that lack a humanoid registry — VRM 0.x fallback), and overwrites the local rotation with `rest_local_rotation * look_at_delta`. The spec defines the delta as a rotation applied to the bone's *rest* rotation, not a delta on top of the current local rotation, so head/eye bones that the motion also animates end up looking at the cursor rather than blending the two sources. Identity deltas are skipped (the rest pose would rotate to itself; cheaper to leave the VRMA result untouched).
- Wire-through in `apps/ene-desktop-v2/src/character/mod.rs::CharacterRenderer::update_motion` (one call site, one line): `model.update_skin_palette(&frame, self.look_at_bone_output.as_ref())`. The diagnostic `look_at_bone_output()` accessor keeps its `#[allow(dead_code)]` (it is read inside `update_motion` rather than elsewhere) but the rationale comment is now "Consumed by `update_motion` (PR4.16)" instead of the PR4.8 placeholder text.
- 4 new unit tests in `crates/ene-vrm/src/model.rs::tests`:
  - `update_skin_palette_applies_look_at_bone_delta_to_head` — a 90° yaw LookAt rotates the head bone's +X axis to -Z.
  - `update_skin_palette_look_at_overrides_vrma_for_head` — LookAt wins over a conflicting VRMA head rotation (the rest-pose semantics).
  - `update_skin_palette_look_at_idempotent_for_zero_delta_or_missing_bones` — identity deltas and missing humanoid entries leave the VRMA result untouched.
  - `update_skin_palette_look_at_ignores_missing_humanoid_bones` — models without a humanoid registry (legacy VRM 0.x) are no-ops rather than panics.
- **Out of scope for PR4.16:** the spec's two-bone IK on the spine → head → eyes chain. The cursor target currently rotates only the head / eyes; shoulders and spine are untouched (matches the legacy `bevy_vrm1` behaviour and the §0 description). The `body_tracking` profile computed on demand by `CharacterRenderer::body_tracking(strength)` is still useful for downstream PRs (e.g. spring-bone secondary motion) but its weights are not yet wired into a solver.

**PR-fix follow-up (post-merge, 2026-06-18)** — choppy look-at + dead drag

Two issues surfaced once the PR4.16 landing was exercised on a real build, both rooted in subtle coordinate-space mistakes in the per-frame wiring:

- **A1 — choppy look-at + camera too low (regressed in the first fix, corrected here).** The original `update_camera_target` was reading `model.nodes.world_positions[chest.node][1]` (the *animated* chest bone world Y from the previous frame's VRMA). With a motion that oscillated the chest, the camera target oscillated, the cursor→world projection oscillated with it, and the head look-at was visibly choppy. The first fix replaced this with `chest.rest.translation[1]` — but that field is the **local** glTF `Node::transform()` (the offset from the parent bone), not the world position. For a humanoid chest deep in the chain (hips → spine → chest → …) the local value is just the offset from the parent (~0.2 m) while the world value is the sum of all ancestors (~1.4 m). The first fix made the camera target ~7× too low, which broke the framing (only the top of the head visible), the look-at (the cursor→world projection lands in a different place than where the head bone actually is), and the drag (the user can't see the model well enough to click on it). The corrected fix is a new private helper `chest_world_rest_y(model, bone) -> f32` that walks the parent chain through `model.nodes.parents` + `model.nodes.rest_local_positions`, accumulating the local Y of each ancestor (the same math `NodeHierarchy::compute_world_transforms` does in the loader, just for a single bone's Y). The pure helper `chest_target_y(world_y, center_y, normalize_scale, model_scale) -> f32` now takes a *world* rest Y. The previously-unused `character_position` parameter is dropped (the camera is world-anchored; the model is positioned within the view via the per-frame `model_matrix`). Five regression tests in `character::camera_target_tests` pin the new behaviour: the hand-computed example for the pure helper, a 3-link parent-chain sum test (`chest_world_rest_y_sums_parent_chain_offsets`), a same-axis-only test that ensures X / Z parent offsets don't bleed into the world Y sum (`chest_world_rest_y_sums_only_y_axis`), a stability test that mutates `world_positions[1]` between two `update_camera_target` calls and asserts the target Y is unaffected (`update_camera_target_is_stable_and_uses_world_rest_y`), and a no-model safety test.
- **B1 — dead drag.** `update_character_collider` (in `apps/ene-desktop-v2/src/physics.rs`) was building the Rapier cuboid from the raw AABB and the `scale` only, ignoring `normalize_scale`, and centring the collider at `translation + raw_center` instead of at `translation`. For Alicia (`normalize_scale ≈ 0.5`, AABB `([-1, 0, -1], [1, 2, 1])`) the collider was ~2× too big and offset ~1 m up. The Rapier raycast from the camera eye through the cursor therefore missed the silhouette, `cursor_over_char_window` always returned `false`, and `on_press_or_release` short-circuited to `DragAction::None` so the drag never started. The fix takes a new `normalize_scale: f32` parameter, sets the local half-extents to `(max - min) * 0.5 * normalize_scale * scale`, and centres the rigid body at `translation` (the model's local origin in world; the `T(-center)` step already re-centres the mesh onto the origin *before* the scale, so the collider is centred at the same point the model is). The runtime caller now passes `model.normalize_scale()`. Two regression tests in `physics::tests` pin the new behaviour: a unit-cube AABB with `normalize_scale = 0.5` produces a `0.5` half-extent cuboid centred at the origin, and a non-symmetric AABB with a `y`-shifted translation centres the collider at the translation, not at `translation + raw_center`.

A drive-by cleanup also lives in the same commit: `Runtime::last_cursor_logical` is renamed to `last_cursor_physical` (the field is a `PhysicalPosition<f64>`, and the cursor path the look-at uses treats it as physical pixels — internally consistent, but the old name lied). The separate `LookAtState::last_cursor_logical: Option<Vec2>` (which really is in logical pixels) keeps its name.

**Subtasks**

> All 6 subtasks have shipped (PR4.3 / PR4.4 / PR4.6 / PR4.7 / PR4.8 / PR4.14 / PR4.16); the bullets below are the original recipe and the matching "PR4.x progress (shipped this PR)" block above each subtask describes the actual implementation.

1. **Expression** — port `bevy_vrm1::vrm::expression`:
   - Each frame, build a `BTreeMap<ExpressionName, f32>` from the latest `EmotionQueue` (driven by `ai_bridge`). PR1's `AppEvent::EmoteToken` channel is the input; PR4 adds the `EmotionQueue` consumer.
   - Multiply the per-expression weight into the per-primitive morph-target buffer.
   - Public API: `VrmModel::set_expression(name, weight)`, `VrmModel::expression_names()`.
2. **LookAt** — port `bevy_vrm1::vrm::body_tracking` (only the look-at part for now):
   - Provide a `LookAtTarget { world_position: Vec3 }` per frame.
   - Solve two-bone IK on the spine → head → eyes chain to point the eyes at the target.
   - Clamp yaw / pitch to the model's VRM-defined ranges.
   - In `apps/ene-desktop-v2/src/character/cursor.rs` (new), convert the OS cursor position to world coordinates in front of the camera at a fixed depth and feed it in. The cursor→world conversion is the legacy `apps/ene-desktop/src/platform.rs::cursor_position_for_window()` logic, lifted and rewrapped.
3. **BodyTracking** — keep a minimal version: only head + eyes follow cursor; shoulder / hand sway is out of scope until we re-add spring bone.
4. **AI bridge integration** — `AiBridge::take_inbox()` is called from `runtime` once per frame; `AppEvent::EmoteToken` items push onto `EmotionQueue`; the queue is then drained and applied to `VrmModel::apply_emotions`.
5. **Drag-to-move (new for v2)** — implement the legacy `character_drag/mod.rs::update_character_drag` flow on top of the new winit event loop:
   - On `MouseButtonInput::Pressed { Left }`, raycast the cursor against every `CharacterRoot`'s transformed AABB. If it hits, start tracking the cursor's world position in `settings.character_state`.
   - Every frame while the button is held, integrate `(cursor_world_pos - last)`.`extend(0.0)` into `settings.character_state.character_position`.
   - On `MouseButtonInput::Released { Left }`, call `settings.mark_dirty()`.
   - The click-through logic (which makes the empty area around the character pass clicks through to the desktop) is **out of scope for PR4**; in PR4 the entire character window is still clickable. PR5 makes only the silhouette clickable.
6. **VRMA playback (was PR5 in the legacy plan, but PR1 in the new v2 plan puts the audio / animation engine inside `apps/ene-desktop-v2::character::vrma` — new module, ported from `bevy_vrm1::vrma`).** Subtasks: load all VRMA files in `assets/characters/<name>/motions/*.vrma` at character-spawn time; play the selected one in `PlayVrma { repeat: Forever, transition_duration: 300 ms, vrma: entity, reset_spring_bones: false }` mode; per-character `default_motion` is honored (already stored in `CharacterConfig`, plumbed in PR2).

**Verification**

- Move the OS cursor: the model's head / eyes track it within the configured clamp angles (PR4.16 wires the evaluator output through `update_skin_palette` so the rotation is actually visible — the pre-PR4.16 `look_at_target()` accessor was reported but not consumed by the skinning math).
- Type "I'm so happy!" in the chat: the model transitions to a happy blend shape.
- Click and drag the character across the screen: `character_position` updates, the model follows, and the new position persists on release.
- Add an automated test in `ene-vrm` that loads a tiny synthetic VRM, sets an expression, and asserts the morph-target buffer reflects the weight.
- `cargo test -p ene-vrm --lib` covers the PR4.16 composition (4 new tests) in addition to the loader / skeleton / expression tests from earlier PR4.x landings.

**Files touched / created (PR4, shipped)**

- `apps/ene-desktop-v2/src/character/{mod,cursor,emotion,drag}.rs` — **new** (PR4.2 / PR4.3 / PR4.4 / PR4.14).
- `apps/ene-desktop-v2/src/character/vrma.rs` — **new** (PR4.14).
- `apps/ene-desktop-v2/src/runtime.rs` — `Runtime::window_event` gains `MouseButtonInput` handling and integrates drag into `state.settings` (PR4.3).
- `apps/ene-desktop-v2/src/state.rs` — consume `AppEvent::EmoteToken` into the `EmotionQueue`; expose `state.drag_state()` (PR4.4).
- `crates/ene-vrm/src/{expression,look_at}.rs` — **new** (PR4.4 / PR4.8).
- `crates/ene-vrm/src/humanoid.rs` — **new** (PR4.7).
- `crates/ene-vrm/src/animation.rs` — **new** (PR4.14).
- `crates/ene-vrm/src/node_constraint.rs` — **new** (PR4.12).
- `crates/ene-vrm/src/spring_bone.rs` — **new** (PR4.13).
- `crates/ene-vrm/src/mtoon.rs` — **new** (PR4.15).
- `crates/ene-vrm/src/{model,renderer,loader}.rs` — extended for PR4.5 (skinning) through PR4.16 (per-joint LookAt composition + skin-palette upload).

### PR5 — v2: click-through (Win32 + Wayland + X11) + offscreen mask

> **Status (2026-06-20):** **PR5.2 (Windows) shipped — via per-bone Rapier colliders (sphere / capsule / Y-capsule, sized from the actual mesh's per-vertex skinning weights, with per-frame translation *and* rotation updates so limb capsules follow the animation).** PR5.1's BVH-trimesh approach was retired (the rest-pose trimesh never tracked the per-frame animation, so face/feet stayed unclickable while the default VRMA was playing). The per-bone collider approach reads the current bone world positions from `VrmModel::nodes::world_positions` (which `update_skin_palette` refreshes each frame) and updates each collider's local translation *and* rotation via Rapier's `set_translation_wrt_parent` / `set_rotation_wrt_parent` — the colliders follow the animation without any per-frame GPU readback. **PR5.6 (collider debug overlay, now also draws capsule wireframes) and PR5.7 (direct-render in `about_to_wait` — bypasses winit 0.30.13 multi-window `RedrawRequested` bug, restores `WaitUntil(deadline)` frame pacer) shipped.** PR5.3+ (Wayland input region, X11 shape, Linux offscreen mask + gizmo) are still pending.

**Objective:** Make the character window clickable only on the visible silhouette; pass everything else through to the desktop.

**Subtasks**

1. **Windows click-through (PR5.2 — shipped 2026-06-19, redesigned twice)** — three pieces, none of which touch Win32 directly:
   - `Window::set_cursor_hittest(allows_input)` from winit. On Windows this toggles `WS_EX_TRANSPARENT` + `WS_EX_LAYERED` and triggers `SetWindowPos(SWP_FRAMECHANGED)`; on other platforms it is a no-op.
   - Global cursor poll via the `device_query` crate (`device_query::DeviceState::new().get_mouse().coords`). Cross-platform; works regardless of which window currently owns focus. Converted to window-local via `Window::outer_position()` + the per-window scale factor.
   - Hit test via per-bone Rapier colliders. `CharacterRenderer::build_character_bone_specs` walks `model.humanoid` once at model load and emits a `BoneShapeSpec` per humanoid bone: `BoneShape::Sphere` for head / jaw / eyes / shoulders / toes (sized to the largest mesh distance from the bone's rest position), `BoneShape::Capsule` along the parent → bone rest direction for upper/lower arms + legs + hands (so an arm's wireframe aligns with the limb, not the world's +Y), `BoneShape::CapsuleY` for neck / spine / chest / upperchest / hips / foot (trunk bones with their local +Y; foot follows the bone's rest world rotation). The radius comes from the actual mesh — `VrmPrimitive` now retains its `Vec<MeshVertex>` (cloned at load time) and `collider::collect_weighted_world_positions_into` filters vertices whose dominant skinning weight is `< VERTEX_WEIGHT_THRESHOLD = 0.25` for the target bone, so a vertex whose dominant weight targets the *shoulder* is not allowed to inflate the *arm* capsule. Bones outside the skin (no inverse_bind) and small bones below `MIN_BONE_RADIUS = 0.025` (fingers) are skipped. `PhysicsWorld::add_character_bone_colliders` registers them as a single kinematic body parked at the world origin; the per-frame `update_character_bone_positions` walks `current_bone_poses` (which reads `model.nodes.world_positions` updated by `update_skin_palette` *and* the per-bone rest → current world rotation) and pushes each new pose into the matching collider via `set_translation_wrt_parent` *and* `set_rotation_wrt_parent` (axis-angle via `Quat::to_axis_angle`). The colliders therefore follow the animation — both position and orientation — without any per-frame GPU readback.
   - The `apps/ene-desktop-v2/src/platform/` module and its `windows_hit_test.rs` were deleted (no per-platform module needed; the winit + device_query + Rapier recipe is fully cross-platform).
2. **Wayland input region** — port `apps/ene-desktop/src/character_drag/linux/region.rs` (the `wayland_client::Connection::from_backend` + `wl_surface::set_input_region` recipe) into a new `apps/ene-desktop-v2/src/platform/wayland_region.rs`.
3. **Wayland offscreen mask capture** — port `apps/ene-desktop/src/character_drag/linux/capture.rs` (the 581-line `R8Unorm` + `Readback::texture` + tile-grid rectangle decomposition) into `apps/ene-desktop-v2/src/platform/wayland_mask_capture.rs`. The `MaskCaptureCamera` lives on a second `wgpu::RenderTarget::Image` (not on the character window's swapchain), reads back the silhouette, and feeds rectangles into the `WaylandInputRegionContext`.
4. **X11 fallback** — port the `CursorOptions::hit_test` path (which is the only X11 mechanism the legacy code uses; the X11 shape extension is not implemented in the legacy code) and the `_NET_WM_STATE_SKIP_TASKBAR` / `_SKIP_PAGER` direct `X11` FFI from `apps/ene-desktop/src/character_drag/linux/taskbar.rs` into `apps/ene-desktop-v2/src/platform/x11_taskbar.rs`.
5. **Linux-only debug overlay** — port the mask-rectangle gizmo from `apps/ene-desktop/src/character_drag/linux/capture.rs::draw_visible_rect_gizmos` to `apps/ene-desktop-v2/src/platform/wayland_mask_gizmo.rs`, gated on the `ui.debug_overlay_visible` flag in the Character settings page.
6. **Mask downsample UI row** — the `Mask Downsample` row on the Character settings page is Linux-only (legacy convention). Wired in PR2's `page_character.rs` (storage), consumed in PR5's `WaylandMaskCaptureState::texture_size` computation.
7. **Frame pacer (PR5.7 — shipped 2026-06-20)** — `Runtime::about_to_wait` now sets `ControlFlow::WaitUntil(Instant + frame_interval)` for capped `target_fps` values and `ControlFlow::Poll` for `0` ("Unlimited"). Earlier blocked by a winit 0.30.13 multi-window + `WaitUntil` bug; that was bypassed by driving the char window's render directly from `about_to_wait` (via `render_char_frame`), decoupling the render from `RedrawRequested` delivery. Replaces the legacy `apps/ene-desktop/src/scene.rs::pace_frame_rate` — see §0 PR5.7.

**Verification**

- On Windows: the empty area around the character passes clicks through to the desktop; clicking on the character silhouette drags it. — **Verified by build + unit tests for `ene-desktop-v2`**; runtime click-through to be exercised on the developer's machine.
- On Linux + Wayland: same as Windows, plus a wayland-layer-shell-aware test (or at minimum a non-regression test for the existing X11 build).
- On Linux + X11: same as Windows, with `_NET_WM_STATE_SKIP_TASKBAR` set so the desktop app does not appear in the taskbar.
- The mask gizmo (debug overlay) shows the extracted silhouette rectangles as purple lineloops when toggled on.

**Files touched / created (PR5, planned + shipped so far)**

- `apps/ene-desktop-v2/src/physics.rs` — **shipped** (PR5.2): `add_character_bone_colliders` (now `BoneShapeSpec`-aware — builds `ColliderBuilder::ball`, `capsule_y` + `rotation`, and applies the spec's `local_rotation` so the collider's local axes are pre-rotated to the bone's "toward-child" direction) + `update_character_bone_positions` (now also `BonePose`-aware — calls `set_rotation_wrt_parent` in addition to `set_translation_wrt_parent` so a swinging arm's capsule follows the limb) + `cast_ray` (global) + `remove_character_colliders`. The PR5.1 trimesh code (`add_character_trimesh`, `cast_ray_at_character`, `update_character_transform`) and the old AABB code (`update_character_collider`) are gone.
- `apps/ene-desktop-v2/src/character/mod.rs` — **shipped** (PR5.2): `build_character_bone_specs` (rest-pose `BoneShapeSpec` per humanoid bone) and `current_bone_poses` (live positions *and* rotations after `update_motion`). The PR5.1 `read_trimesh_data` and the `TrimeshBuildData` type alias are removed.
- `apps/ene-desktop-v2/src/character/collider.rs` — **new** (PR5.2 vertex-weight refit): `BoneShape` (`Sphere { radius }` / `Capsule { half_height, radius, axis: Vec3 }` / `CapsuleY { half_height, radius }`), `BoneShapeSpec { local_position, local_rotation, shape }`, `BonePose { position, rotation }`, `MIN_BONE_RADIUS = 0.025`, `VERTEX_WEIGHT_THRESHOLD = 0.25`, `compute_bone_specs(model, normalize_scale) -> Vec<BoneShapeSpec>` (walks `model.humanoid` + the per-primitive `VrmPrimitive.vertices` to size each bone from the actual mesh), `compute_rest_world_positions`, `collect_weighted_world_positions_into` (the threshold filter), `fit_limb_capsule` (parent → bone rest direction), `fit_trunk_capsule` (world +Y), `fit_bone_axis_capsule_y` (bone's rest world rotation), `fit_sphere`, `humanoid_parent_node`, `strip_side_prefix` (drops VRM `_L` / `_R` / `.L` / `.R` suffixes before matching against the bone-name table), plus unit tests for the weight filter, identity inverse_bind, and humanoid-parent resolution.
- `apps/ene-desktop-v2/src/runtime.rs` — **shipped** (PR5.2 + PR5.7): `device_state: device_query::DeviceState` field, the `update_char_window_cursor_and_hittest` helper, `set_cursor_hittest` toggle, per-frame `update_character_bone_positions` (replaces the per-frame trimesh transform update; now also updates rotation), a `cast_ray` + entity-id check for the press predicate, the `render_char_frame` method (PR5.7 — drives the char render directly from `about_to_wait`, sidestepping the winit 0.30.13 multi-window `RedrawRequested` bug), and the `WaitUntil(Instant + frame_interval)` / `Poll` frame pacer at the end of `about_to_wait` (restored once the render path no longer depends on `RedrawRequested`).
- `apps/ene-desktop-v2/src/raycast_debug.rs` — **shipped** (PR5.6 capsule-aware overlay): `build_collider_lines` now reads the collider's `position().rotation` and emits a Y-axis capsule wireframe (top + bottom caps + `SPHERE_LONGITUDES` meridians via `capsule_wireframe_lines_into`) for any `Collider::as_capsule()` in addition to the existing sphere path for `Collider::as_ball()`. The test suite gained `capsule_collider_produces_wireframe_lines` (regression guard: future refactors that drop capsule support will not silently leave limbs and the trunk invisible).
- `crates/ene-vrm/src/model.rs` — `VrmPrimitive` now retains `vertices: Vec<MeshVertex>` (PR5.2 refit) so the collider builder can read per-vertex skinning weights after the GPU upload without a CPU round-trip.
- `crates/ene-vrm/src/loader.rs` — `process_mesh_node` clones the `vertices` into `VrmPrimitive` after the GPU upload.
- `crates/ene-vrm/src/lib.rs` — re-exports `MeshVertex` so `apps/ene-desktop-v2` can name it without depending on `ene-vrm::model::*` internals.
- `crates/ene-vrm/src/debug_renderer.rs` — adds `capsule_wireframe_lines_into` + `DebugRenderer::push_capsule_wireframe` (the 2-cap + meridian wireframe consumed by the PR5.6 overlay).
- `apps/ene-desktop-v2/src/character/drag.rs` — docstring updated; `is_dragging` accessor is no longer `#[allow(dead_code)]`.
- `apps/ene-desktop-v2/src/platform/` — **removed** in PR5.1 (no per-platform module needed). Will return for the Wayland input-region module (PR5.3).
- `apps/ene-desktop-v2/Cargo.toml` — **shipped** (PR5.1): `device_query = "2"` added; `windows-sys` no longer needs the `Win32_UI_Shell` feature. Linux target gains `wayland-client` and `raw-window-handle` for PR5.3.
- `apps/ene-desktop-v2/src/ui/page_character.rs` — wire `Mask Downsample` row (Linux-only `cfg`).

### PR5.5 — rename + delete legacy

> **Status:** Not started. **Gated on PR1–PR5 all being "Shipped" in §0.**

**Objective:** Replace the legacy Bevy `apps/ene-desktop` with the v2 binary, in a single commit, without breaking the workspace.

**Subtasks**

1. `git mv apps/ene-desktop-v2 apps/ene-desktop` (after deleting the legacy dir to avoid a name clash; or vice versa, depending on which is faster).
2. `rm -rf apps/ene-desktop-LEGACY` (the Bevy sources).
3. In `apps/ene-desktop/Cargo.toml` and the `Cargo.toml` workspace `[members]` list, drop the Bevy deps (already gone in v2) and the legacy `tray-icon` / `png` / `windows-sys` / `wayland-client` cfg-gates (consolidated into v2's single `Cargo.toml`).
4. Update the `description` in `apps/ene-desktop/Cargo.toml` to reflect the new stack.
5. Verify `cargo build --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` are all green with **no** `--exclude`.
6. Update `docs/architecture/wgpu-migration.md` to mark the "v2 grows to full parity" journey as complete; keep §22 (implementation notes) as a historical record.
7. Mark §0 table: all PR1–PR5 rows as "Shipped"; PR5.5 as "Shipped" (the rename is the final commit).

**Verification**

- `cargo run -p ene-desktop` launches the v2 binary with the full feature set (tray, settings, VRM, drag, click-through, etc.).
- The workspace has **zero** Bevy deps left (`cargo tree | rg bevy` returns nothing).
- The legacy `patches/bevy_winit/` and `apps/tw-test/` are already gone (PR0).

### PR6+ — Deferred Work (carried over from the legacy plan)

- **PR6** — Spring-bone simulator now drives the v2 character every frame. The parser + `SpringBoneSimulator` landed in `crates/ene-vrm/src/spring_bone.rs` (PR4.13); A.6 wires it into `apps/ene-desktop-v2/src/character/mod.rs` so the simulator is built once on model load, then stepped per-frame inside `update_motion` after `update_skin_palette` has produced the live world transforms. The simulator's output is written back into `model.nodes.local_rotations` for the affected joints; the next `update_skin_palette` picks them up. PR5.2's per-bone collider debug overlay draws the spring-bone colliders alongside the bone colliders, so the visual confirmation is already there.
- **PR7** — FXAA post-process (A.7). The settings UI row ships in PR2 (`page_graphics.rs`); the actual wgpu FXAA pipeline lands here. `crates/ene-vrm/src/post_process.rs` holds the post-processor (intermediate texture + FXAA pipeline + uniform buffer); `crates/ene-vrm/src/shaders/fxaa.wgsl` is the public-domain FXAA Quality preset. `CharacterRenderer::render` now takes a `swapchain_size` + `swapchain_format` + `AntialiasingMode` and either draws straight to the swapchain (AA = Off) or renders into the intermediate texture and runs FXAA into the swapchain. SMAA / TAA stay as follow-up PRs (the user opted to ship FXAA only).
- **PR8** — Drag-while-clicked polish (smoother multi-monitor handling, sub-pixel position rounding).
- **PR9** — Per-character `default_expression` now flows end-to-end. `CharacterState` carries the in-memory value (defaulted to `"neutral"` on a fresh install), `sync_to_store` writes it into the per-character `CharacterConfig` on disk, and `load_per_character_settings` reads it back. The cycle buttons on the Character page (and the runtime WASD hotkey) push the new character's default expression into the renderer's `EmotionQueue` on every actual character switch. The legacy Bevy code persisted the field but always wrote the empty string (a known bug); the migration path treats an empty on-disk value as `"neutral"`.

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

### 5.2b New in `apps/ene-desktop-v2` (PR0 + PR1, shipped)

This is the **actual** file layout on disk today. The full split below lands incrementally as PR2–PR5 progress; once the migration is done the v2 crate will be moved to `apps/ene-desktop` and these files will be replaced by the §5.2 layout.

**Layout (8 source files, ~1.5k LoC as of writing):**

```text
apps/ene-desktop-v2/
├── Cargo.toml        # winit, wgpu, pollster, bytemuck, glam, tracing, tracing-subscriber,
│                     # egui, egui-wgpu, egui-winit, ene-core, ene-config, ene-tool-proto,
│                     # tokio, parking_lot, png, tray-icon, ctor, serde, serde_json, thiserror
│                     # Linux target: gtk (with tray-icon/gtk), wayland-client (PR5)
│                     # Windows target: windows-sys (PR5)
└── src/
    ├── main.rs       # tracing init + tokio multi-thread runtime + AppState::with_channel + EventLoop::run_app
    ├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
    ├── runtime.rs    # Runtime, CharacterWindow, UiWindow, RectRenderer, ApplicationHandler, AcquireError
    ├── state.rs      # AppState (gpu, settings, ai, tray, event_rx), AppStateError, with_channel
    ├── events.rs     # AppEvent, AiStreamUpdate, AppEventSender/Receiver (tokio mpsc)
    ├── settings.rs   # CharacterSettings (plain struct, ported from legacy app_config.rs)
    ├── ai_bridge.rs  # AiBridge wrapping EneHandle, tokio tasks: pump_events, bootstrap
    └── tray.rs       # TrayHandle (Windows: GetMessageW thread; Linux: GTK pump + receiver thread)
```

**File-by-file responsibility:**

- **`apps/ene-desktop-v2/Cargo.toml`** — see the comment block above. PR1 added `ene-core`, `ene-config`, `ene-tool-proto`, `tokio`, `parking_lot`, `png`, `tray-icon`, `ctor`, `serde`, `serde_json`, `thiserror`. Linux target adds `gtk = "0.18.2"` and enables the `gtk` feature on `tray-icon`. PR5 will add `wayland-client` (Linux) and `windows-sys` (Windows).
- **`apps/ene-desktop-v2/src/main.rs`** — `tracing_subscriber::fmt` install + `tokio::runtime::Builder::new_multi_thread().enable_all().build()` + `runtime.enter()` guard (held for the duration of `EventLoop::run_app`) + `AppState::resolve_paths()` (resolves `assets_dir` via `ene_config::ensure_resource_dirs()`, reads `args[1]=vrm` / `args[2]=vrma` overrides) + `pollster::block_on(GpuContext::new)` + `CharacterSettings::discover(assets_dir, default_vrm)` + `AppState::with_channel(gpu, settings, &handle)` + `EventLoop::new` + `Runtime::new(state, sender).run_app`.
- **`apps/ene-desktop-v2/src/gpu.rs`** — `GpuContext`, `pick_format_and_alpha`, `backend_options` (DX12 / `DxgiFromVisual` on Windows, `Backends::PRIMARY` elsewhere). Unchanged from PR0.
- **`apps/ene-desktop-v2/src/runtime.rs`** — `Runtime { state, event_tx, transparent, char_window, ui_window, rect }`. `ApplicationHandler`:
  - `resumed` — `state.init_tray()` → create the `CharacterWindow` and the `UiWindow` (PR0 + PR2 code, unchanged).
  - `window_event` — dispatches by `WindowId` (char vs ui), handles `Space` / `Escape` / `close` / `Resized` / `ScaleFactorChanged` / `RedrawRequested` / `KeyboardInput`.
  - `about_to_wait` — drain `event_rx`: on `Tray::OpenSettings` set `state.settings.ui.settings_window_visible = true`; on `Ai(_)` push the update to `state.ai.take_inbox()`; on `Quit` `state.save()` and `event_loop.exit()`; on `EmoteToken(_)` stash for PR4; always call `state.settings.flush_if_dirty(Some(&char_name))`; on Linux call `state.tray.tick_gtk()`; redraw both windows.
- **`apps/ene-desktop-v2/src/state.rs` (new, PR1)** — `AppState { gpu, settings: Arc<RwLock<CharacterSettings>>, ai: Arc<AiBridge>, tray: Option<TrayHandle>, event_rx: UnboundedReceiver<AppEvent> }`. The `Sender` half is returned to `main` and then handed back to `Runtime`. `with_channel(gpu, settings, &handle) -> (Self, AppEventSender)`. Methods: `init_tray`, `ai_run(&str)`, `save`, `request_quit`, `resolve_paths`. `AppStateError` (thiserror).
- **`apps/ene-desktop-v2/src/events.rs` (new, PR1)** — `pub enum AppEvent { Tray(TrayAction), Ai(AiStreamUpdate), EmoteToken(String), Quit }`, `pub enum AiStreamUpdate { TextDelta, ToolCallStart, ToolCallResult, PermissionRequired, UserInputRequired, TaskProgress, Finished, Error }` (mirrors `ene_core::EneEvent` minus the internal `StatusChanged` / `SessionSplit`), `pub type AppEventSender = tokio::sync::mpsc::UnboundedSender<AppEvent>`, `pub type AppEventReceiver = tokio::sync::mpsc::UnboundedReceiver<AppEvent>`.
- **`apps/ene-desktop-v2/src/settings.rs` (new, PR1)** — `pub struct CharacterSettings { pub assets_dir: PathBuf, pub characters: Vec<CharacterEntry>, pub graphics: GraphicsSettings, pub character_state: CharacterState, pub ui: UiState, pub ai: AiConfig, pub store: Arc<parking_lot::RwLock<ene_config::ConfigStore>> }`. Includes the `GraphicsSection` (declared via `ene_config::define_config!(settings, "desktop", DesktopSection { graphics })`), `ShadowQuality` / `AntialiasingMode` enums via `define_label_enum!`, `CharacterEntry`, `CharacterState`, `UiState`, `AiConfig`. Methods: `discover`, `current_entry`, `current_character`, `current_motion`, `current_character_card`, `sync_card_path`, `clamp_runtime_values`, `save_per_character_settings`, `load_per_character_settings`, `select_character`, `save`, `mark_dirty`, `sync_to_store`, `load_from_file`, `flush_if_dirty`. Helpers: `read_cli_paths`, `cycle_mask_render_downsample`, `cycle_target_fps`, `cycle_shadow_quality`, `cycle_antialiasing_mode`, `target_fps_label`, `normalize_*`. Constants: `DEFAULT_CHARACTER_NAME`, `DEFAULT_VRM_PATH`, `DEFAULT_VRMA_PATH`, `APP_ID`, `WINDOW_WIDTH`, `WINDOW_HEIGHT`, `SETTINGS_WINDOW_WIDTH`, `SETTINGS_WINDOW_HEIGHT`, `MASK_RENDER_DOWNSAMPLE_CHOICES`, `TARGET_FPS_CHOICES`, `SHADOW_QUALITY_CHOICES`, `ANTIALIASING_MODE_CHOICES`, plus their `DEFAULT_*` constants.
- **`apps/ene-desktop-v2/src/ai_bridge.rs` (new, PR1)** — `pub struct AiBridge { handle: EneHandle, event_rx: broadcast::Receiver<EneEvent>, inbox: parking_lot::Mutex<VecDeque<AiStreamUpdate>>, latest_response: parking_lot::Mutex<String>, processing: AtomicBool }`. Constructor spawns two tokio tasks: `pump_events` (loops `try_recv` on `handle.subscribe()`, translates `EneEvent → AiStreamUpdate` and pushes into `inbox`; `SpecialToken` events are scanned for `<|emo:NAME|>` via `ene_core::extract_emotion_from_token` and forwarded as `AppEvent::EmoteToken`) and `bootstrap` (calls `handle.load_config()` then `handle.load_character()` in sequence; logs `warn!` on failure). Public methods: `run(&str)`, `cancel()`, `drain(&AppEventSender)`, `take_inbox()`, `latest_response()`, `processing()`, `set_processing(bool)`.
- **`apps/ene-desktop-v2/src/tray.rs` (new, PR1)** — `pub struct TrayHandle { _tray_icon: Option<TrayIcon>, gtk_pump: bool, sender: AppEventSender }`. On Windows: spawns a dedicated thread that calls `TrayIconBuilder::new().with_icon(...).with_menu(...).build()`, then runs a hand-rolled `GetMessageW` / `TranslateMessage` / `DispatchMessageW` loop forever; `mem::forget`s the icon so the thread keeps it alive. On Linux: builds the icon on the main thread (must happen on the GTK main thread), spawns a separate thread that polls `TrayIconEvent::receiver()` and `MenuEvent::receiver()` and forwards `OpenSettings` / `Quit` to `AppEventSender`. `tick_gtk(&mut self)` calls `gtk::main_iteration_do(false)` once, gated on `gtk::events_pending()`. PNG decode accepts `Rgba` / `Rgb` / `Grayscale` / `GrayscaleAlpha`; on `Indexed` or any decode error, falls back to a 32×32 opaque blue square `(0, 128, 255, 255)`. Menu IDs: `ene.settings` ("Settings") and `ene.quit` ("Quit"); left-click on the tray icon also fires `OpenSettings` (matches the legacy UX).

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
| `wgpu` | 29 | wgpu core (bumped from 27 → 29 in the v2 scaffold) | yes (workspace dep; consumed by `apps/ene-desktop-v2`) |
| `winit` | 0.30 | event loop and windowing | yes (workspace dep; consumed by `apps/ene-desktop-v2`) |
| `egui` | 0.34 | immediate-mode UI | yes (workspace dep; consumed by `apps/ene-desktop-v2` for the PR2 `UiWindow`) |
| `egui-wgpu` | 0.34 | egui → wgpu renderer | yes (workspace dep; consumed by `apps/ene-desktop-v2` for the PR2 `UiWindow`) |
| `egui-winit` | 0.34 | egui input integration | yes (workspace dep; consumed by `apps/ene-desktop-v2` for the PR2 `UiWindow`) |
| `glam` | 0.33 | linear algebra (bumped from 0.29 → 0.33 with `bytemuck` feature) | yes (consumed by `apps/ene-desktop-v2`) |
| `gltf` | 1.4 | VRM / glTF parsing | workspace dep declared, not yet consumed (PR3) |
| `encase` | 0.12 | shader-compatible struct packing (UBOs) | workspace dep declared, not yet consumed (PR3) |
| `bytemuck` | 1 | safe `Pod`/`Zeroable` casts | yes (consumed by `apps/ene-desktop-v2`) |
| `pollster` | 0.4 | minimal `block_on` for startup | yes (consumed by `apps/ene-desktop-v2`) |
| `raw-window-handle` | 0.6 | wgpu surface creation | workspace dep declared, not yet consumed (PR3) |
| `parking_lot` | 0.12 | non-poisoning `RwLock` for `CharacterSettings` / `AiBridge` inbox | yes (consumed by `apps/ene-desktop-v2` for PR1 `state.rs` / `ai_bridge.rs`) |
| `png` | 0.18 | tray icon decode (`Rgba` / `Rgb` / `Grayscale` / `GrayscaleAlpha`) | yes (consumed by `apps/ene-desktop-v2` for PR1 `tray.rs`) |
| `tray-icon` | 0.24 | system tray, used by both `apps/ene-desktop` (legacy) and `apps/ene-desktop-v2` (PR1) | yes (consumed by `apps/ene-desktop-v2` for PR1 `tray.rs`; the legacy app also keeps using it until the PR5.5 rename) |

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

- The glTF mesh primitive's `targets` field carries per-vertex deltas for blend shapes. `crates/ene-vrm/src/expression.rs` defines `PrimitiveMorphs { primitive_id, targets: Vec<MorphTarget>, name_to_slot, target_count, vertex_count }`; the loader populates it from `primitive.reader(...).read_morph_targets()` POSITION displacements, normalised by the loader's `scale` **only** (NOT `(p - center) * scale` — morph deltas are linear, not absolute, so translating them by `-center` would drag every weighted vertex toward the model centre). See `loader::normalize_morph_offset` and its regression test `morph_offset_is_not_translated_by_model_centre`.
- Morph names are **not** exposed on the public `gltf::mesh::MorphTarget` view. The loader reads them out of the `VRMC_vrm` extension tree: `Document::extensions()["VRMC_vrm"]["expressions"].{preset,custom}.<name>.morphTargetBinds[*]` (each bind is `{node, index}`, resolved via `Node::mesh().index()`). The name is bound to **every primitive** of the referenced mesh (the spec says "all primitives must share the same morphTarget"); targets not referenced fall back to `morph_target_<i>`.
- We allocate one storage buffer per morph-bearing primitive: `morph_offsets: array<vec3>` of length `target_count * vertex_count`, indexed as `morph_offsets[target_index * vertex_count + vertex_index]`. Primitives without morph targets share a single dummy layout with `target_count = 0`; the shader's `if (target_count > 0u)` early-out skips the storage lookup.
- For each expression name (e.g. `happy`, `sad`, `blink`), we pre-populate `ExpressionState`/`ExpressionLayer::weights` keys from the model's resolved names. `VrmModel::set_expression(name, weight)` is the runtime's write path; the renderer reads it via `model.expressions().weights.get(name)`.
- The vertex shader takes `morph_meta: MorphMeta { vertex_count, target_count, _pad0, _pad1, weights: array<vec4<f32>, 16> }`. The packed `weights` array stores 4 morph weights per `vec4` (so 16 vec4s = 64 slots per primitive). It applies `position += sum_t( weights[t/4][t%4] * morph_offsets[t * vertex_count + vidx] )`.
- **Deferred (PR4.5+):** normal / tangent morph displacements, multi-target blend-shape graphs (e.g. `blink_l + blink_r → blink`), look-at `expression` mode (writes `lookLeft/Right/Up/Down` directly into the weights map). The look-at `expression` mode shipped in PR4.8 (`crates/ene-vrm/src/look_at.rs::LookAtEvaluator`); the per-category `mouth_mul` / `blink_mul` / `look_at_mul` math was already in the `apply_overrides` evaluator in `crates/ene-vrm/src/expression_override.rs` (which `ExpressionLayer::apply_overrides` calls every frame from `update_motion`). A.9 added two regression tests for the blend-shape graph: an `overrideBlink = block` source zeroes the full blink family (`blink`, `blinkLeft`, `blinkRight`) and an `overrideLookAt = block` zeroes the full gaze family (`lookUp`, `lookDown`, `lookLeft`, `lookRight`). Only **normal / tangent morph displacements** remain as a future loader change (the source `POSITION` / `TANGENT` morph targets are read today but only POSITION is currently used by the renderer; a TANGENT-aware shader pass is out of scope).

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
| `wgpu` re-exports change minor versions in 0.x releases | Medium | Low | Pin to `wgpu = "29.x.y"` exactly until 29.x is stable. |
| `egui-wgpu` 0.34 API differs from 0.39 used in `bevy_egui` | Medium | Low | Examples in the egui-wgpu repo are the source of truth. We write a small `EgUiWgpuHelper` once. |
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

> **Superseded by PR0 / §22.3.** Every "correct diagnosis" in this section is itself out of date; the actual root cause is that wgpu (27, and the same applies to 29) does not auto-read the `WGPU_DX12_PRESENTATION_SYSTEM` env var. The application has to pass the desired `Dx12SwapchainKind` directly to `BackendOptions::dx12::presentation_system` in `wgpu::Instance::new` (v2 does this in `apps/ene-desktop-v2/src/gpu.rs::backend_options`; Bevy 0.18 does the equivalent in `bevy_render/src/renderer/mod.rs:201`). See §22.3 for the full story and the working recipe.

The earlier PR1 attempts (DX12 env var + `WS_EX_LAYERED` nudge, Vulkan swap) and the intermediate "missing `WS_EX_NOREDIRECTION_BITMAP`" diagnosis are all preserved in git history but no longer relevant to anyone reading this file. Kept as a one-line pointer so cross-references from outside the doc do not 404.

### 22.2 PR1 file-level status (as of writing)

> **Status:** PR1 is **Shipped**. The legacy `apps/ene-desktop/` (Bevy 0.18) is intentionally **untouched** per the new §0.1 / §4 policy (v2 grows to full parity, then a single rename at PR5.5). The two crates build side-by-side.

| Action | File / directory | State |
|--------|------------------|-------|
| **New** | `crates/ene-vrm/Cargo.toml` | Shipped (PR1 step 2) |
| **New** | `crates/ene-vrm/src/lib.rs` | Shipped (PR1 step 2 — `pub fn version()` stub + one unit test) |
| **New** | `apps/ene-desktop-v2/Cargo.toml` | Shipped (PR0 + PR1) |
| **New** | `apps/ene-desktop-v2/src/{main,gpu,runtime}.rs` | Shipped (PR0 — full v2 smoke) |
| **New** | `apps/ene-desktop-v2/src/state.rs` | Shipped (PR1) |
| **New** | `apps/ene-desktop-v2/src/events.rs` | Shipped (PR1) |
| **New** | `apps/ene-desktop-v2/src/settings.rs` | Shipped (PR1) |
| **New** | `apps/ene-desktop-v2/src/ai_bridge.rs` | Shipped (PR1) |
| **New** | `apps/ene-desktop-v2/src/tray.rs` | Shipped (PR1) |
| **Workspace** | `Cargo.toml` rendering-stack section | Shipped (PR0 + PR1: `wgpu`, `winit`, `egui*`, `glam`, `pollster`, `bytemuck`, `parking_lot`, `png`, `tray-icon`) |
| **Workspace** | `Cargo.toml` `[patch.crates-io] bevy_winit` | **Shipped** — the local `patches/bevy_winit/` patch was removed in PR0; the legacy `apps/ene-desktop` now uses upstream `bevy_winit` 0.18 from crates.io |
| **Rewritten** | `apps/ene-desktop/src/{main,app_config,ai_bridge,tray}.rs` | **Not started** — legacy Bevy binary untouched. These files stay in place until the PR5.5 rename. |
| **Rewritten** | `apps/ene-desktop/Cargo.toml` | **Not started** — still Bevy 0.18 |
| **Deleted** | `apps/ene-desktop/src/{scene,character,platform}.rs` | **Not started** — deferred to PR5.5 (rename) |
| **Deleted** | `apps/ene-desktop/src/{settings_ui,character_drag}/` | **Not started** — deferred to PR5.5 (rename) |
| **Deleted** | `patches/bevy_winit/` | **Shipped** — patch directory and `[patch.crates-io] bevy_winit` in workspace `Cargo.toml` are both gone |
| **Deleted** | `apps/tw-test/` | **Shipped** — Bevy testbed removed; the §22.3 cross-reference is preserved as a historical note only |

The high-level intent ("rename `apps/ene-desktop-v2/` to `apps/ene-desktop/` once v2 has full legacy feature parity") is unchanged; the timing of the rename is now end-of-PR5 (PR5.5) instead of end-of-PR1. See §22.4 below for the full PR1 implementation notes.

### 22.3 PR0 — Minimum v2 transparency smoke

> **Status:** Shipped. This is the smallest possible `apps/ene-desktop-v2` that proves the §8.1 Windows transparency recipe works end-to-end on this machine. It supersedes the (incorrect) §22.1 diagnosis.

**v2 layout (3 source files, ~840 lines as of writing):**

```text
apps/ene-desktop-v2/
├── Cargo.toml        # winit, wgpu, pollster, bytemuck, glam, tracing, tracing-subscriber
└── src/
    ├── main.rs       # tracing-subscriber init + EventLoop::run_app
    ├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
    └── runtime.rs    # Runtime, WindowSlot, RectRenderer, ApplicationHandler, AcquireError
```

The full 7-module split planned in §5.2 (`runtime/{mod,input,surface,window_slot,loop,rect}.rs`, `gpu/{mod,depth,surface_format}.rs`, `platform/{mod,...}.rs`) was collapsed into the three files above. Reasons in the §22.3 "Files in this PR" block further down.

**Goal:** Get a winit + wgpu (originally 27, current build uses 29) window on Windows to (a) be transparent (DWM honors the swapchain's per-pixel alpha), (b) draw a single colored rectangle, and (c) toggle between transparent / opaque with `Space`, exit on `Escape`. No egui, no VRM, no AI bridge. Pure rendering smoke.

**Recipe that finally works (do all four together):**

1. **`wgpu::Dx12SwapchainKind::DxgiFromVisual`** — passed directly to `BackendOptions::dx12::presentation_system` in `apps/ene-desktop-v2/src/gpu.rs::backend_options` and forwarded to `wgpu::Instance::new`. This is the wgpu (originally 27, current build uses 29) DX12 backend option that creates the swapchain from the HWND's visual and is required for per-pixel alpha. The `WGPU_DX12_PRESENTATION_SYSTEM` env var has no effect on its own — wgpu 27/29 only consults it inside `Dx12SwapchainKind::from_env()`, which v2 does not call. Without `DxgiFromVisual`, the wgpu DX12 surface is created as `SurfaceTarget::WndHandle` and `Surface::get_capabilities` returns only `[CompositeAlphaMode::Opaque]` (the wgpu-hal DX12 adapter file referenced at the time of writing was `wgpu-hal-27.0.4/src/dx12/adapter.rs:1006-1018`), which is the root cause of the persistent "opaque black" symptom in all earlier PR0 runs.

2. `WindowAttributesExtWindows::with_no_redirection_bitmap(true)` — set **in the `WindowAttributes` builder** in `apps/ene-desktop-v2/src/runtime.rs::window_attributes`, regardless of `transparent`. This adds `WS_EX_NOREDIRECTIONBITMAP` (0x00200000) to the HWND's exstyle at create time. **This is the piece that §22.1's `force_layered_window` approach was missing.** Setting only `WS_EX_LAYERED` (via `with_transparent(true)`) makes wgpu advertise `PreMultiplied` correctly, but DWM still composites the window through the redirection bitmap and shows the background as opaque black, because the swapchain's per-pixel alpha is not being read. Both styles must be present:

   ```text
   WS_EX_NOREDIRECTIONBITMAP = 0x00200000   (added by with_no_redirection_bitmap(true))
   ```

   > **WARNING — updated after PR0 closeout.** On winit 0.30.x, `with_transparent(true)` does **not** add `WS_EX_LAYERED` to the HWND by itself (only `WindowFlags::TRANSPARENT` is set, and that flag is consulted only in the legacy DWM blur-behind path which is skipped when `with_no_redirection_bitmap(true)` is also set). The only places winit 0.30 adds `WS_EX_LAYERED` to the exstyle are `with_ignore_cursor_events(true)`. **Do not try to fix the missing bit with `SetWindowLongPtrW(WS_EX_LAYERED)` from a post-create hook** — on the developer's PR0 machine, doing so on top of `WS_EX_NOREDIRECTIONBITMAP` caused DWM to revert the window to opaque-black composition (the legacy layered path apparently overrides the non-redirected path). The `with_no_redirection_bitmap` call in `window_attributes` is the only knob that should be touched.

3. `CompositeAlphaMode::PreMultiplied` in `SurfaceConfiguration` — **picked directly** by `gpu::pick_format_and_alpha` from the platform, **not** from `SurfaceCapabilities::alpha_modes`. The earlier implementation iterated the caps looking for `PreMultiplied` / `PostMultiplied` and fell back to `Opaque` when neither was listed; that was the root cause of the persistent "opaque black" symptom in earlier PR0 runs — the surface quietly degraded to `Opaque` instead of telling us it could not honour the request. The current implementation just returns `CompositeAlphaMode::PreMultiplied` on Windows / Linux and `CompositeAlphaMode::PostMultiplied` on macOS, matching `apps/tw-test` exactly. If the surface really does not support it, `Surface::configure` is called with an unsupported alpha mode and wgpu 29 (same as wgpu 27) will fail the next `get_current_texture`; the `Runtime::window_event` `AcquireError::Reconfigure` arm logs a clear `WARN` and reconfigures, so the failure is no longer silent. `WindowSlot::new` also emits an immediate `WARN` if the requested alpha mode is not in `SurfaceCapabilities::alpha_modes` — that is the single log line to look for to confirm the surface was misconfigured.

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
- `apps/ene-desktop-v2/Cargo.toml` — slimmed to `winit`, `wgpu`, `pollster`, `bytemuck`, `glam`, `tracing`, `tracing-subscriber` (env-filter + fmt), plus **PR2-merged** `egui` / `egui-wgpu` / `egui-winit` (consumed by `runtime::UiWindow`). No `raw-window-handle`, no `windows-sys`, no `ene-core`, no `tokio`, no `tray-icon`.
- **Layout: 3 source files, ~840 lines as of writing.** `src/main.rs` (tracing init + event loop), `src/gpu.rs` (`GpuContext` + `pick_format_and_alpha` + DX12 backend options), `src/runtime.rs` (`Runtime` + `CharacterWindow` + `UiWindow` + `RectRenderer` + `ApplicationHandler` impl + `AcquireError`).
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
- The wgpu surface format picker still falls back to `Opaque` if the host's DXGI does not advertise `PreMultiplied`. In that case `Space` still toggles `decorations` and the clear color, but DWM composites the background as opaque.

---

## 22.4 PR1 — v2 tray + AI bridge + `AppState` + persistence + CLI

> **Status:** **Shipped.** v2 now boots a tokio runtime, owns a tokio-driven `EneHandle` actor, a system tray (`tray-icon` 0.24), and the `CharacterSettings` schema (ported from the legacy Bevy `Resource` to a plain `Arc<parking_lot::RwLock<…>>` struct). The legacy `apps/ene-desktop` (Bevy 0.18) is deliberately untouched per the new §0.1 / §4 policy. See the §0 table for the summary and §4 PR1 for the step-by-step plan.

### Why "v2 grows to full parity, then rename" instead of the old "PR1 is the deletion step"

The original plan called for "PR1 is the deletion step" — strip Bevy from `apps/ene-desktop`, move the v2 sources into it. The risk with that policy is that at any given commit the workspace can be in a state where a feature works in one binary and not the other, or where neither binary launches. The new policy (see §0.1) is that **both binaries build side-by-side throughout the entire migration**; v2 grows incrementally through PR1–PR5+; and the rename + deletion of the legacy sources happens in a single commit at the end of PR5 (PR5.5). The two-crate coexistence is now the project's default mode, not a transitional state.

This is a developer-experience change only. From a user's perspective, the migration is invisible until PR5.5, at which point the Bevy binary disappears and the v2 binary is the one that launches.

### Module map (8 files, ~1.5k LoC as of writing)

```text
apps/ene-desktop-v2/src/
├── main.rs       # tracing init + tokio runtime + AppState::with_channel + EventLoop::run_app
├── gpu.rs        # GpuContext, pick_format_and_alpha, backend_options (DX12 / DxgiFromVisual)
├── runtime.rs    # Runtime, CharacterWindow, UiWindow, RectRenderer, ApplicationHandler
├── state.rs      # AppState (gpu, settings, ai, tray, event_rx), AppStateError, with_channel
├── events.rs     # AppEvent, AiStreamUpdate, AppEventSender/Receiver (tokio mpsc)
├── settings.rs   # CharacterSettings (plain struct, ported from legacy app_config.rs)
├── ai_bridge.rs  # AiBridge wrapping EneHandle, tokio tasks: pump_events, bootstrap
└── tray.rs       # TrayHandle (Windows: GetMessageW thread; Linux: GTK pump + receiver thread)
```

### Key design decisions

1. **`CharacterSettings` is a plain struct, not a `Resource`.** The legacy Bevy app used `bevy::ecs::resource::Resource` which gives you automatic `Arc`-wrapping and `Deref` ergonomics. v2 has no Bevy; we use `Arc<parking_lot::RwLock<CharacterSettings>>` instead. The `Arc` is cloned into `AiBridge` (so the bridge can read `ai.ai.character` / `ai.ai.*` to drive the actor's reconfigure) and into `Runtime` (so the winit event loop can mutate `ui.settings_window_visible` on `Tray::OpenSettings`). `parking_lot` is used instead of `std::sync::RwLock` to avoid poison-related panics on a winit keypress that races with a background config save.

2. **`AppEvent` is a `tokio::sync::mpsc` channel, not a `bevy::Message` bus.** The legacy Bevy app used `bevy::prelude::Messages` for inter-system communication. v2 has no Bevy ECS, so the producer–consumer pattern is a `mpsc::UnboundedSender<AppEvent>` (cloned into the tray thread and into the AI bridge's tokio task) plus an `UnboundedReceiver<AppEvent>` owned by `Runtime`. The drain happens once per frame in `Runtime::about_to_wait`, so the channel is effectively the same as a frame-bounded event bus but without Bevy's macro overhead. The `tokio` mpsc (not `std::sync::mpsc`) was chosen because (a) `AiBridge::pump_events` runs inside a tokio task and tokio mpsc is the natural cross-task channel, and (b) `tokio::sync::mpsc::UnboundedSender` is `Send + Sync` and supports `try_send` from any thread (including the Windows tray thread and the Linux GTK thread).

3. **Tray on Windows uses a dedicated `GetMessageW` thread; on Linux, the icon is built on the main thread and a separate thread polls the `tray-icon` receivers.** The split is forced by GTK's threading model: `tray-icon` with the `gtk` feature holds GTK objects (icons, menus) that are **not** `Send`; they must live on the main thread. On Windows there is no such constraint, so the legacy code's "spawn a thread that owns the icon and runs a hand-rolled Win32 message pump" recipe carries over verbatim. The Linux thread reads `TrayIconEvent::receiver()` and `MenuEvent::receiver()` and forwards the parsed action to `AppEventSender`; the icon itself never leaves the main thread.

4. **The `<|emo:NAME|>` token is parsed in `AiBridge::pump_events`, not in a Bevy `system`.** Legacy `character::enqueue_ai_special_tokens` did the same thing on a Bevy schedule. v2 does it in the bridge: every `EneEvent::SpecialToken` is fed through `ene_core::extract_emotion_from_token(token)` and, if it matches, forwarded as `AppEvent::EmoteToken(String)`. The consumer (the `EmotionQueue` in `apps/ene-desktop-v2::character::emotion`) lands in PR4; PR1's job is just to plumb the channel.

5. **Per-frame `flush_if_dirty` lives in `Runtime::about_to_wait`.** The legacy `settings_ui::auto_save_config` ran as a Bevy system in the `SettingsWindowContextPass` schedule. v2 has no schedules, so the equivalent is a single `state.settings.flush_if_dirty(Some(&char_name))` call after the `event_rx` drain. The `ConfigStore` debounces the actual `std::fs::write` (it tracks the dirty flag internally), so calling it every frame is safe and does not write to disk unless something actually changed.

6. **CLI is `args[1]=vrm` / `args[2]=vrma` with a hard-coded fallback.** `AppState::resolve_paths` reads `std::env::args().nth(1)` and `.nth(2)`, falling back to `DEFAULT_VRM_PATH` / `DEFAULT_VRMA_PATH` (matching the legacy). The v2-only difference is that the second value is **also** used — legacy `read_cli_paths` returned `(vrm, vrma)` but `main` only used `vrm` (legacy comment: "The motion path comes entirely from the discovered character's `motion_paths` list"). v2 mirrors that behavior; PR3 will add explicit VRMA override when VRM/VRMA loading lands.

### Verification (PR1 closeout)

- `cargo build -p ene-desktop-v2` — green.
- `cargo clippy -p ene-desktop-v2 -- -D warnings` — green. The 23 dead-code errors that surfaced during development (most of the new public API is `#[allow(dead_code)]`-annotated because the consumers land in PR2) are scoped and will be removed as PR2–PR5 land.
- `cargo clippy --workspace --exclude ene-desktop -- -D warnings` — green. Legacy `apps/ene-desktop` clippy errors are **pre-existing** and out of scope (the live `render_settings_window` in the legacy code is a "test" stub, so `page_ai_page` / `page_character_page` / `page_graphics_page` are reported as dead — left for the legacy maintainers).
- `cargo test --workspace` — 198 passed, 6 ignored. No new tests added by PR1 (all the action is plumbing; the first real behavior tests land with the settings UI in PR2).
- Manual smoke on Windows: `cargo run -p ene-desktop-v2` shows the system tray icon "ene", left-click opens the UI window (PR0 + PR2 demo), the "Settings" tray menu sets `ui.settings_window_visible` (a no-op visual until PR2 wires the page), the "Quit" tray menu exits cleanly. The character window still shows the red-quad smoke from PR0. The legacy `cargo run -p ene-desktop` is unchanged and still launches the Bevy 0.18 build.
- Manual smoke on Linux (developer machine): same behavior, with the GTK pump running in `Runtime::about_to_wait` (a single `gtk::main_iteration_do(false)` per frame is enough to keep the tray responsive; the icon never leaves the main thread).

### Known limitations (carried over to PR2+)

- `AiBridge::run`, `AppEvent::Quit`, `AppEvent::EmoteToken`, and most `AiStreamUpdate` variants are `#[allow(dead_code)]`. The consumers (settings page, hotkeys, drag, etc.) land in PR2–PR4.
- The settings UI is still the PR0 + PR2 demo (`"Hello from separate egui window!"` and a click counter). The real 3-page settings UI is PR2.
- The system tray's "Settings" menu sets `ui.settings_window_visible = true`, but nothing renders that flag until PR2.
- Drag-to-move and click-through are PR4 / PR5; the v2 character window is fully clickable in PR1.
- VRM / VRMA / LookAt / expressions / spring-bone are all still on the legacy Bevy build; v2's character window is the red-quad smoke. The migration of those features is PR3 / PR4 / PR6.

## 22.5 PR2 — v2 settings UI (3 pages) + hotkeys + per-character config

### Module map

```
apps/ene-desktop-v2/src/
├── character_state.rs   (NEW; 38 lines)  AnimationControl, EmotionCommand, EmotionQueue
├── settings.rs          (MODIFIED)        +PendingPermission, +PendingUserInput, +QuestionDraft
├── settings_ui/         (NEW; 5 files, ~750 lines)
│   ├── mod.rs           (PageKind, SettingsUi, apply_egui_visuals)
│   ├── input.rs         (SettingsInputState + sync_from_settings)
│   ├── widgets.rs       (SettingsAction enum, apply_action dispatcher)
│   ├── page_ai.rs       (provider / key / embedding / memory / chat / latest-response)
│   ├── page_character.rs (char / motion / play-pause / lookat / scale / pos / 6 expression buttons; Linux: debug overlay, mask downsample)
│   └── page_graphics.rs (target FPS / shadow / AA cycle rows)
├── runtime.rs           (MODIFIED)        F1 / WASD / Space / Esc; show/hide; auto-popup in about_to_wait
└── main.rs              (MODIFIED)        +mod character_state; +mod settings_ui;
```

### Key design decisions

- **No Bevy `Resource` / `Message` / `System`.** The page functions take `(&mut egui::Ui, &mut CharacterSettings, &mut AnimationControl, &Arc<AiBridge>, &mut SettingsInputState, &mut EmotionQueue, f64)`. The runtime's `about_to_wait` passes them in. The `Arc<AiBridge>` lets the AI page call `ai.run(&input)` directly; the legacy Bevy code used `MessageWriter<EneRequestEvent>` because of Bevy's `EventWriter` constraint.
- **F1 is a character-window key, not an egui key.** The character window's `KeyboardInput` handler matches `NamedKey::F1` and toggles `ui.settings_window_visible`. This works whether the UI window is open or hidden, matching the legacy's global `ButtonInput<KeyCode>::just_pressed(KeyCode::F1)`.
- **WASD hotkeys use `physical_key` (`KeyCode`)** instead of `logical_key` (`Key::Character`). This preserves the QWERTY → AZERTY ergonomics the legacy `KeyCode::KeyW` family had. The hotkey map is gated on `cw_char_window_has_focus(cw) && current_page == PageKind::Character`.
- **Auto-popup is two-step.** `about_to_wait` collects pending permission / user-input events from the bus into local `Option<…>` accumulators first, then writes them into `UiState` after the loop. This avoids borrowing `self.state.settings.ui.pending_user_input` mutably while iterating `self.state.event_rx`.
- **`EmotionQueue` lives in `SettingsUi`, not in `AppState`.** The queue is UI-side state (the buttons push, the renderer pops). The runtime just reads `now_secs = started_at.elapsed().as_secs_f64()` from the `UiWindow`'s `SettingsUi`.
- **The 6 expression buttons deliberately use a fixed `hold_secs: 4.0`.** The legacy code had a comment on `character.rs:71` saying "future enhancement: `<|DELAY:n|>` token would set `hold_secs = n`". v2 keeps the constant; the comment moves to `character_state.rs`.

### Verification

- `cargo build -p ene-desktop-v2` — green.
- `cargo clippy -p ene-desktop-v2 -- -D warnings` — green.
- `cargo clippy --workspace --exclude ene-desktop -- -D warnings` — green.
- `cargo test --workspace` — 198 passed, 6 ignored (37 suites). No new tests (the `CharacterSettings` round-trip test lands with PR3 alongside the VRM fixtures).
- Legacy `apps/ene-desktop` (Bevy 0.18) still builds (`cargo build -p ene-desktop` — 0 errors, 31 pre-existing warnings from the dead `render_settings_window` test stub).

### Known limitations (deferred)

- The "pending permission / question" dialogs are not rendered yet; the data path is wired. The next PR that touches this area will add the dialogs.
- Numeric row `TextEdit` fields are display-only; the +/- buttons are the primary input. The legacy had re-parse-on-Enter.
- `AiBridge::processing` (the legacy's `ene.processing` flag) does not yet gate the chat input. Will land with PR3.
- The "Settings" tray menu now carries an optional page focus: `TrayAction::OpenSettings { page: Option<PageKind> }`. The tray click / menu paths still pass `None` (legacy behavior), but the runtime can also push `Some(PageKind::Ai)` when a `PermissionRequired` or `UserInputRequired` event arrives, so the AI page (and the dialogs A.5 will render) is on screen by the time the user reaches for the keyboard.


