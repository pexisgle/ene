# `ene-desktop` ECS Migration Progress

This document tracks the in-progress refactor of `apps/ene-desktop` from a
procedural `hecs`-based implementation to a `bevy_ecs 0.19` +
`bevy_app 0.19` architecture. The end state is a `bevy_ecs::App` that
owns all per-frame logic; `hecs` and the `winit` `Runtime` are left as
thin shells.

## Status

| Phase | Scope | State |
|------:|-------|-------|
| 0 | Add `bevy_ecs` / `bevy_app` dependencies and an empty `App` skeleton (`app.rs`, `schedule.rs`). | ✅ Done |
| 1 | Promote the per-process singletons (`FrameState`, `ExitRequested`, `TokioHandle`) to bevy `Resource`s. `AppState` now holds `app: App`; `Runtime::about_to_wait` ticks it. | ✅ Done |
| 2 | Convert the legacy `AppEvent` bus into typed bevy `Message`s. Add a `First`-stage `pump_legacy_events` system that drains the receiver and writes into the message buffers. | ✅ Done |
| 3 | `CharacterPlugin` — assemble the per-character components (`VrmModelHandle`, `MotionState`, `SpringBoneState`, `CharacterCamera`, `LookAt`, `EmotionChannel`, `BoneColliders`, `CharacterTransform`, plus `Transform` / `GlobalTransform`) into a `CharacterBundle` and spawn the entity in `Startup`. | ✅ Done |
| 4 | `PhysicsPlugin` — refactor `PhysicsWorld` to drop the `entity_to_*` `HashMap`s. `register_character_colliders` returns a `CharacterColliderRegistration`; the per-bone handles live on the entity as `PhysicsBody` / `PhysicsColliders` / `PhysicsColliderStaticOffsets` / `PhysicsColliderStaticRotations` / `PhysicsColliderRestRotations`. `attach_bone_colliders_system` runs in `Startup`; `step_physics_system` runs in `Update`. | ✅ Done |
| 5 | `UiPlugin` — split the legacy `SettingsUi` struct into bevy `Component`s (`UiWindow`, `UiPage`, `UiInputDrafts`, `UiAnimation`, `UiEmotionQueue`, `UiStartedAt`, `UiStateComponent`). `apply_action` now takes a bevy `World` + `Entity`; the page render functions read / write components via `world.get` / `world.get_mut`. The `SettingsActionEvent` message is registered for Phase 6+ consumption. | ✅ Done |
| 6 | `PlatformPlugin`, `TrayPlugin`, `AiPlugin` — Phase 6 work. The tray menu, the AI bridge pump, and the Linux Wayland / X11 input-region state migrate into bevy resources + systems. | ✅ Done |
| 7 | Render path integration — see "Phase 7 — Render Path Integration" below. The skeleton `RenderPlugin` + 7 stub systems + `GpuDeviceHandle` / `CharWindowSurface` / `RenderFrameContext` scaffolding landed in 7.2, but the 7.2-final body migration proved infeasible (`CharacterRenderer` is `!Send + !Sync`; the `tick_gtk_system` drain-only pattern is the only safe split and the render path has no bevy-specific aggregation to do). The skeleton was deleted in the 7.2-final cleanup pass. | ✅ Done (cancelled body migration) |
| 8 | Polish — clippy + test + reduce `Runtime::about_to_wait` to < 10 lines (Phase 5: ~ 90 lines). | ⏳ Pending |
| 9 | Documentation sync — English (`docs/`) + Japanese (`docs/ja/`). | 🔄 In progress |

## Key Architecture Rules

* `#[expect(...)]` is used instead of `#[allow(...)]` for every
  not-yet-wired field; this keeps unfulfilled expectations visible
  during the refactor.
* The `bevy_ecs` `Message` trait requires a corresponding
  `Messages<T>` resource — registered via `app.add_message::<T>()`.
* `PluginGroup::build` returns a `PluginGroupBuilder`; the
  `PluginGroup` itself must derive `Default`.
* Sets are configured with `configure_sets`, not `add_systems`.
* `app.update()` runs all five stages (`First` / `PreUpdate` /
  `Update` / `PostUpdate` / `Last`) plus the `Startup` schedule
  exactly once on the first call.
* `IntoScheduleConfigs` lives in `bevy_ecs::schedule`, not in
  `bevy_ecs::prelude`.
* Test worlds must call `world.init_resource::<Messages<T>>()` for
  every `T` accessed via `MessageWriter` / `MessageReader`.

## File Layout (post-Phase 5)

```text
apps/ene-desktop/src/
├── app.rs                  # DesktopPlugins (PluginGroup) + CorePlugin
├── schedule.rs             # AppSet + configure_schedule / configure_startup
├── component/
│   ├── character.rs        # CharacterBundle + 10 per-character components
│   ├── physics.rs          # PhysicsBody / Colliders / static offsets
│   ├── transform.rs        # Transform / GlobalTransform
│   └── ui.rs               # SettingsUiBundle + 7 per-UI components
├── event/
│   ├── ai.rs               # AiTextDelta / AiStreamFinished / AiPermissionRequested / AiUserInputRequested / EmoteToken
│   ├── input.rs            # PointerMoved / PointerButton / KeyboardKey
│   ├── lifecycle.rs        # WindowResized / WindowCloseRequested
│   ├── settings.rs         # OpenSettings
│   └── ui_action.rs        # SettingsActionEvent
├── plugin/
│   ├── character_plugin.rs
│   ├── physics_plugin.rs
│   └── ui_plugin.rs        # UiPlugin + spawn_settings_ui_window
├── resource/
│   ├── event_channels.rs   # EventChannels (legacy bridge)
│   ├── exit.rs             # ExitRequested
│   ├── frame_state.rs      # FrameState
│   ├── pending_actions.rs  # PendingActions (legacy bridge)
│   ├── physics.rs          # PhysicsWorldResource
│   └── tokio.rs            # TokioHandle
├── system/
│   ├── event_pump.rs       # pump_legacy_events
│   └── physics.rs          # attach_bone_colliders_system + step_physics_system
├── settings_ui/            # egui rendering (page_character / page_graphics / page_ai / page_debug / widgets / input)
│                            — all `apply_action` paths now take bevy `World` / `Entity`
└── runtime.rs              # winit + egui glue; calls `app.update()` each frame
```

## Verification

* `cargo build -p ene-desktop` — clean
* `cargo clippy --workspace -- -D warnings` — clean
* `cargo test -p ene-desktop` — 116 passed
* `cargo fmt --all` — clean

## Phase 6+ Plan (the remaining work)

### Phase 6 — Platform / Tray / AI

* Lift the Linux `WaylandInputRegionContext` / `LayerShellState` /
  `X11Context` state out of `PlatformState` into bevy `Resource`s.
  Add a `PlatformPlugin` with systems that refresh input regions
  on `Update` when the `MaskCaptureState` is `Some`.
* Add a `TrayPlugin` that owns the `TrayHandle` and translates
  menu events into `OpenSettings { page }` messages.
* Add an `AiPlugin` that owns the `AiBridge` and a bevy
  `Resource<PendingAiEvents>`. The current `pump_legacy_events`
  system already drains the receiver into `PendingActions`; this
  phase wires the dispatcher to translate `PendingActions` into
  the typed `Message`s that already exist.
* Split the `SettingsAction` 40+ variants in
  `settings_ui/widgets.rs::apply_action` into per-action systems
  in `system/ui_actions/`. Each system reads / writes one
  `SettingsActionEvent` and mutates the relevant component on
  the `UiWindow` entity.

### Phase 7 — Render Path Integration

* `WindowPlugin` owns the `winit::Window` and the `wgpu::Surface`
  as `NonSend` resources.
* `RenderPlugin` owns the `wgpu::Device` and `wgpu::Queue` as
  `NonSend` resources.
* Acquire / encode / submit / present become `Last`-stage systems
  in `AppSet::Render` / `AppSet::Present`.
* `CharacterPlugin::finish` materialises the `VrmRenderer` once
  the `NonSend<wgpu::Device>` is available.
* The `hecs::World` is finally deleted; `character_entity` is
  looked up via `Query<(Entity, &CharacterRoot)>`.

### Phase 8 — Polish

* Clippy pass with `-D warnings` and every unfulfilled
  `#[expect(dead_code)]` resolved.
* `Runtime::about_to_wait` shrinks from ~ 90 lines to < 10 (the
  remaining lines forward the per-frame actions into the
  `EventChannels` resource).
* Integration tests for the new `SettingsActionEvent` consumer
  system.

### Phase 9 — Documentation

* Sync the English `docs/architecture/ene-desktop-ecs-migration.md`
  with the final `apps/ene-desktop/src/` layout.
* Translate to Japanese at `docs/ja/architecture/ene-desktop-ecs-migration.md`
  (this file is the source of truth).
* Update `docs/applications/desktop.md` (and the Japanese mirror)
  to reflect the new bevy-based architecture.

## Phase 6+ Detailed Plan

The plan in this section is the operational follow-up to the
prose above. It locks in plugin ordering, system boundaries,
test counts, and the retirement sequence for `PendingActions`
and the legacy `hecs::World`. The "End state" target is a
`Runtime::about_to_wait` body of < 10 lines (per Phase 8) and
a fully bevy-driven per-frame path.

### Phase 6 — Platform / Tray / AI (detailed)

**Scope**: replace the in-line body of `Runtime::about_to_wait`
lines 304–370 (the `PendingActions` drain, the GTK tick, the
camera transform copy-back, the `device_query` hit-test glue)
with three plugins + a small dispatcher system set. End state:
`about_to_wait` calls `app.update()` and then `render_char_frame()`
/ `ui_window.render_frame()`; everything between is a system.

**Plugin additions** (added to `DesktopPlugins` in `app.rs`
*after* `UiPlugin` so the per-UI components already exist when
these systems first run):

```text
PluginGroupBuilder::start::<Self>()
    .add(CorePlugin)
    .add(CharacterPlugin)
    .add(PhysicsPlugin)
    .add(UiPlugin)
    .add(PlatformPlugin)   // NEW
    .add(TrayPlugin)       // NEW
    .add(AiPlugin)         // NEW
```

#### PlatformPlugin

* **Resources added** (all `#[cfg(target_os = "linux")]`):
  * `WaylandInputRegionContext` (renamed from
    `PlatformState::wayland_region`; loses the `Arc<Mutex<_>>`
    wrapper since bevy resources are already `Sync`).
  * `X11Context` (renamed from `PlatformState::x11_ctx`).
  * `LayerShellState` (renamed from `PlatformState::layer_shell`).
  * `LayerShellFreeze(bool)` (renamed from
    `PlatformState::layer_shell_freeze`).
  * `MaskCaptureState` (renamed from
    `PlatformState::mask_capture`).
  * `MaskReadbackWorker` (renamed from
    `PlatformState::mask_readback_worker`).
  * `LastAppliedInputRects` (renamed from
    `PlatformState::last_applied_input_rects`).
  * `LastInputSource` (renamed from
    `PlatformState::last_input_source`).
  * On non-Linux builds each resource is `Default::default()`
    and the systems no-op.
* **Construction**: `PlatformPlugin::build` is empty; the fields
  are populated lazily in `Runtime::resumed` (still the only
  place we have a real `winit::Window` to pass to
  `WaylandInputRegionContext::try_new`). At that call site we
  replace the legacy
  `self.state.platform.wayland_region = Some(ctx)` with
  `world.insert_resource(ctx)`.
* **Systems** (in `AppSet::Settings` so they run *after* the AI
  / UI has potentially triggered a page change, and *before*
  `Animation` so the next-frame hit-test sees fresh input-region
  state):
  * `refresh_input_region_system` — if `MaskCaptureState` is
    `Some`, ask the readback worker for new rects and push them
    via `WaylandInputRegionContext::apply` and
    `X11Context::apply_shape_rects`. Writes
    `LastAppliedInputRects` and `LastInputSource` for the F9
    overlay.
  * `apply_linux_click_through_system` — replaces
    `update_char_window_cursor_and_hittest` lines 1174–1182.
    Reads the cursor from a new `CursorState` resource (see
    below), reads the latest `MaskCaptureState` rects, and
    toggles `WaylandInputRegionContext` / `X11Context` based
    on `cursor_over || drag_is_dragging || layer_shell_freeze`.
  * `tick_gtk_system` — replaces the tray/GTK pump block at
    lines 349–354 *and* 367–370. Reads `TrayHandle` from a
    new resource (see TrayPlugin). Runs only when
    `tick_gtk: ResMut<PendingActions>` was set *or* a
    `TickGtk` message was written this frame.
* **New resource**: `CursorState` (replaces
  `Runtime::last_cursor_physical`). The `MessageWriter<CursorMoved>`
  / `MessageReader<CursorMoved>` already exist as the
  `PointerMoved` message (Phase 2); `CursorState` is updated
  by a 3-line `update_cursor_state_system` in `AppSet::Input`.
* **Removal**: `PlatformState` collapses to a
  `#[cfg(target_os = "linux")] struct PlatformAdapters`
  containing only handles that *winit* needs to keep alive
  but bevy does not own (e.g. the `MaskReadbackWorker`'s join
  handle). The struct stays because `Runtime::resumed` still
  holds the surface and the `MaskReadbackWorker` lifetime is
  tied to the `wgpu::Device`.

#### TrayPlugin

* **Resource added**: `TrayHandleResource(#[cfg(target_os = "linux")] TrayIcon)`
  — the linux-side drop guard migrates from
  `TrayHandle::_icon`. On Windows the resource is `()` (the
  icon lives in the dedicated Win32 pump thread and is
  `mem::forgotten` there, same as today).
* **Construction**: `TrayPlugin::build` does nothing. The
  tray is built by `Runtime::resumed` calling
  `TrayHandle::new(event_tx.clone())` and then
  `world.insert_resource(TrayHandleResource(_icon))` exactly
  once (guarded by a `Once` flag or a
  `!world.contains_resource::<TrayHandleResource>()` check).
* **Systems**:
  * `pump_tray_events_system` — runs in
    `AppSet::EventDispatch` (the `First`-stage set). Drains
    the `tray_icon` global receivers (already in a background
    thread today, so this system is a thin shim that
    translates their cross-thread `AppEvent::Tray` sends into
    the typed `OpenSettings { page }` /
    `SettingsActionEvent::Quit` messages). Replaces the
    50 ms `std::thread::sleep` busy loop *only if* we can
    move that pump onto the main thread; for now the
    background thread keeps running and this system is a
    no-op placeholder.
  * `tick_gtk_system` lives in `PlatformPlugin` (above) and
    reads `TrayHandleResource` to call `tick_gtk()`. The
    Linux-only `gtk` import stays in `tray.rs`; the system
    just calls a method on the resource.
* **Removal**: the `AppEvent::Tray(_)` arm in
  `translate_event` (`system/event_pump.rs:72-79`) becomes
  the *only* place that handles tray events. The
  `OpenSettings` message it writes is consumed by a new
  `open_settings_system` in `AppSet::Settings` that flips
  the `UiWindow` component's `settings_window_visible` flag
  (replacing `Runtime::show_settings_window`).

#### AiPlugin

* **Resources added**:
  * `AiBridgeResource(Arc<AiBridge>)` — moved from
    `AppState::ai`. `AiBridge::new` is *not* called by the
    plugin; `Runtime::resumed` calls it once with the
    `event_tx`, then
    `app.world_mut().insert_resource(AiBridgeResource(ai.clone()))`.
  * `ProcessingFlag(Arc<AtomicBool>)` — moved from
    `AiBridge::processing`. The bridge still owns the
    canonical `Arc`; the resource just holds a clone so the
    egui page can
    `world.resource::<ProcessingFlag>().0.load(...)` without
    going through `Arc<AiBridge>`.
* **No new systems** in this phase. `pump_legacy_events`
  already drains `AiStreamUpdate` into the typed `AiTextDelta`
  / `AiPermissionRequested` / `AiUserInputRequested` /
  `AiStreamFinished` messages; Phase 6 adds **consumers** of
  those messages in `UiPlugin` (see below).
* **Removal**: `PendingActions::ai_text_deltas` is read once
  more in Phase 6 by a `apply_ai_text_deltas_system` in
  `AppSet::Settings` that appends to
  `UiStateComponent::ai_latest_response`. From Phase 7
  onwards the egui page reads `MessageReader<AiTextDelta>`
  directly and `ai_text_deltas` is dead.

#### Per-action UI system split (closes the Phase 5 `SettingsActionEvent` TODO)

`settings_ui/widgets.rs::apply_action` currently has a 40+
variant `match` that mutates the per-entity `UiStateComponent`,
`CharacterSettings`, and `AiBridge`. Phase 6 splits it into
`system/ui_actions/` modules:

| Module | Variants |
|---|---|
| `prev_next_character.rs` | `PrevCharacter`, `NextCharacter` |
| `prev_next_motion.rs` | `PrevMotion`, `NextMotion` |
| `animation_toggle.rs` | `Play`, `Pause`, `Restart` |
| `ai_send_cancel.rs` | `AiSend(input)`, `AiCancel` |
| `ai_permission.rs` | `AiAnswerPermission { request_id, decision }` |
| `ai_user_input.rs` | `AiSubmitUserInput { request_id, response }` |
| `window_toggle.rs` | `ToggleSettingsWindow`, `ToggleColliderDebug`, `ToggleInputRegionDebug`, `ToggleMaskGizmo`, `ToggleLayerShellFreeze` |

Each system takes a `MessageReader<SettingsActionEvent>`, a
`Query<(&mut UiStateComponent, &mut UiInputDrafts, &mut UiPage), With<UiWindow>>`,
and a `Res<AiBridgeResource>`. `UiPlugin::build` adds them in
`AppSet::Settings`. `dispatch_settings_action` in `runtime.rs`
becomes a 2-line forwarder:

```rust
fn dispatch_settings_action(&self, action: SettingsAction) {
    let mut world = self.state.app.world_mut();
    world.write_message(SettingsActionEvent(action));
}
```

`world.write_message` is the
`bevy_ecs::message::MessageWriter::write` shorthand and is the
only public call into bevy from the winit handler. **All
other state mutations happen inside the new systems.**

#### Phase 6 test plan

* `unit: pump_legacy_events` — existing 7 tests in
  `system/event_pump.rs:174-262` keep working; add 2 for the
  new `PendingActions::tick_gtk` round-trip and the new
  `AppEvent::EmoteToken(now_secs)` path with `Some(...)`
  (the existing test pins `now_secs = None`).
* `unit: open_settings_system` — assert that sending an
  `OpenSettings` message flips `UiWindow::settings_window_visible`
  from `false` to `true` (and stays `true` on a second send).
* `unit: apply_ai_text_deltas_system` — feed 3 `AiTextDelta`
  messages, assert the joined buffer equals the
  concatenation.
* `integration: PlatformPlugin` — start a fresh `App`,
  register `PlatformPlugin`, write a `CursorMoved` + a
  `MaskCaptureState::new_with_rect(rect)`, run
  `app.update()`, assert `LastAppliedInputRects` contains
  `rect` and `LastInputSource` is `MaskCapture`.
* `integration: AiPlugin` — start a fresh `App`, build a real
  `tokio::runtime::Runtime`, spawn an `AiBridge`, send a
  `Run` command, await the `EneEvent::TextDelta`, push
  through `EventChannels::tx`, run `app.update()`, assert
  `Messages<AiTextDelta>` has one entry.

Target test count after Phase 6: **75 + 7 = 82** (3 platform
integration, 2 ai integration, 2 unit).

### Phase 7 — Render Path Integration (detailed)

**Scope**: move the body of `Runtime::render_char_frame`
(lines 714–982) and the `ui_window` render branch (lines
466–499) into `AppSet::Render` / `AppSet::Present` systems.
End state: `PendingActions` is deleted, `hecs::World` is
deleted, `character_entity` is a bevy `Entity`.

**End-of-Phase-7 about_to_wait** (target < 10 lines, modulo
error handling):

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    self.state.app.update();
    if self.state.app.world().resource::<ExitRequested>().0 {
        self.state.save();
        event_loop.exit();
        return;
    }
    self.render_char_frame();
    if let Some(uw) = self.ui_window.as_mut() {
        uw.render_settings_frame(self.state.app.world_mut());
    }
    self.set_frame_deadline(event_loop);
}
```

#### WindowPlugin / RenderPlugin

> **Status: cancelled in Phase 7.2-final (cleanup pass).**
>
> The skeleton `RenderPlugin` + 7 stub systems +
> `GpuDeviceHandle` / `CharWindowSurface` / `RenderFrameContext`
> scaffolding landed in Phase 7.2 (commit history) and were
> deleted in Phase 7.2-final after the body migration proved
> infeasible. The `character.update_*` / `character.render` /
> `cw.with_surface_view` calls cannot move into bevy systems
> because `CharacterRenderer` is `!Send + !Sync` (holds
> `wgpu::TextureView`, `wgpu::ShaderModule`, `VrmRenderer`,
> etc.) and `wgpu::Device` / `wgpu::Queue` are `!Send` on
> Vulkan. The drain-only pattern from
> [`system::tray_tick::tick_gtk_system`] (Phase 7.5) is the
> only safe split: bevy systems own *input aggregation* and
> the runtime owns the *GPU submission*. For the render path
> there is no bevy-specific aggregation to do — every per-frame
> input is already managed by `Runtime`, so the honest answer
> was to delete the skeleton entirely.
>
> The rest of this section is preserved as the design that
> *was planned* and may become relevant if a future Phase
> splits the renderer into a worker thread (would require
> `Arc<Mutex<CharacterRenderer>>` or a dedicated render
> thread). For now, the design is dormant.

* **WindowPlugin resources** (`NonSend`):
  * `NonSend<winit::Window>(Arc<winit::Window>)` — the
    character window.
  * `NonSend<winit::Window>` for the UI window, keyed by a
    `WindowKind` enum component on a per-window entity so
    systems can pick one with a
    `Query<(&WindowKind, &NonSend<winit::Window>))>`.
  * `NonSend<wgpu::Surface<'static>>` per window.
  * `NonSend<wgpu::SurfaceConfiguration>` per window.
* **RenderPlugin resources** (`NonSend`):
  * `NonSend<wgpu::Device>` (the clone Arc'd into the mask
    readback worker stays — the worker holds its own
    `Arc<wgpu::Device>` per the Phase 4 refactor).
  * `NonSend<wgpu::Queue>`.
  * `GpuContext` becomes a `Resource` with the `Instance` +
    `Adapter` (which *are* `Send`); the `Device` / `Queue`
    move to `NonSend`.
* **Why `NonSend` not `Local`**: the `wgpu::Device` is
  `!Send` on Vulkan, and bevy's default executor is
  multi-threaded. `NonSend` keeps the accessor on the main
  thread automatically and the `wgpu::Device` does not need
  a `Mutex`.
* **Construction**: `WindowPlugin` and `RenderPlugin` are
  added to `DesktopPlugins` *before* `CharacterPlugin` so
  the `NonSend<wgpu::Device>` exists when
  `CharacterPlugin::finish` runs.

#### System split for the char window render

| Stage | System | Lines replaced in `render_char_frame` |
|---|---|---|
| `AppSet::Render` | `update_camera_target_system` | 740–744 |
| `AppSet::Render` | `update_motion_system` | 753–755 (writes `MotionState` component) |
| `AppSet::Render` | `update_look_at_system` | 757–768 |
| `AppSet::Render` | `build_debug_lines_system` | 783–914 (collider, mask, input-region lines) |
| `AppSet::Render` | `render_vrm_system` | 770–782, 916–942 |
| `AppSet::Render` | `render_mask_system` (Linux only) | 946–967 |
| `AppSet::Present` | `acquire_present_system` | surface acquire / submit / present (replaces the `result` match at 968–982) |
| `AppSet::Present` | `apply_input_region_system` | moved here from PlatformPlugin so the click-through reflects what was *actually drawn* this frame |

The 5 collider/raycast variants in
`update_char_window_cursor_and_hittest` (lines 1085–1185)
become one system, `update_cursor_state_system`, that takes
a `MessageWriter<CursorMoved>`, a `Res<DeviceState>`, and a
`Res<Time>`. The Windows-only cursor-over raycast becomes a
20-line block inside the system; the Linux-only input-region
apply becomes a 5-line block that calls into
`apply_linux_click_through_system` (still in PlatformPlugin).

#### PendingActions retirement sequence

The fields retire in this order. Each step adds a test that
proves the system-based path produces the same output as the
legacy path:

1. `quit` → `ExitRequested.0` (Phase 1, **done**). The runtime
   reads `ExitRequested` directly.
2. `open_settings` → `OpenSettings` message +
   `open_settings_system` (Phase 6, see above).
   `PendingActions::open_settings` is `#[expect(dead_code)]`
   and removed in 7.3.
3. `ai_text_deltas` → `AiTextDelta` message +
   `apply_ai_text_deltas_system` (Phase 6). Removed in 7.3.
4. `ai_permission` → `AiPermissionRequested` message +
   `apply_ai_permission_system` (Phase 6). Removed in 7.3.
5. `ai_user_input` → `AiUserInputRequested` message +
   `apply_ai_user_input_system` (Phase 6). Removed in 7.3.
6. `emotion_commands` → already populated by
   `pump_legacy_events` and consumed by
   `Runtime::apply_emotions` (line 362). A new
   `apply_emotions_system` in `AppSet::Animation` moves the
   `settings_ui.emotion_queue.push(cmd)` and
   `character.apply_emotions(...)` calls into bevy. Removed
   in 7.4.
7. `tick_gtk` → `TickGtk` message + `tick_gtk_system`
   (Phase 6). Removed in 7.4.
8. **7.5 — `PendingActions::default()` is removed; the
   `EventChannels` resource is replaced by typed messages
   pushed directly from the AI bridge + tray thread.** The
   bridge's `event_tx` is deleted; the bridge writes into
   a `MessageWriter<AiTextDelta>` etc. via an
   `AsyncMessageSender` resource (bevy 0.14+ pattern; bevy
   0.19 supports it natively via `IntoSystem`). At this
   point `EventChannels` is `#[expect(dead_code)]` and the
   next phase deletes the file.

#### hecs retirement sequence

1. `AppState::world: hecs::World` is used in
   `runtime.rs:139-145` and `runtime.rs:374-382` (the
   camera transform copy-back) and `runtime.rs:590-595`
   (the `physics_world.resource` access that is *already*
   going through bevy). **Phase 4 already removed the
   `entity_to_*` HashMaps** but the `hecs::World` itself
   is still in use.
2. `AppState::character_entity: hecs::Entity` is used in
   `runtime.rs:378`
   (`world.query_one_mut::<&mut Transform>(self.state.character_entity)`).
   The new path is a
   `Query<(&mut Transform, With<CharacterRoot>)>` in
   `AppSet::Settings`; the `app.update()` in
   `about_to_wait` runs that system before the render path.
3. **7.6 — `AppState::world` and `AppState::character_entity`
   are deleted.** `hecs` is removed from `Cargo.toml` and
   from the import list. `state.rs` shrinks by ~25 lines.

#### Phase 7 test plan

* `unit: update_cursor_state_system` — feed 3 `PointerMoved`
  messages with `(x, y)`, assert
  `CursorState.physical == (x, y)`.
* `unit: build_debug_lines_system` — given a
  `LastRaycastHit { collider: 3, point: ... }` and a 4-bone
  `humanoid` map, assert the line list contains exactly the
  expected `DebugLine` count.
* `integration: CharFrameRender` — too expensive to write
  today (it needs a real `wgpu` device + a real VRM).
  **Skip the full integration test**; add a `compile_only`
  test that asserts `RenderPlugin` builds against a `wgpu`
  headless device. Document the manual smoke test (run
  `cargo run -p ene-desktop --release`, click the character
  window, press F1, F3, F8, F9) in the PR template.
* `unit: PendingActions::default` no longer compiles (the
  runtime no longer imports it) — gate this with a
  `#[test] fn pending_actions_resource_unused()` that
  asserts
  `app.world().get_resource_by_id(...)` returns `None`
  after the new dispatcher systems are registered.

Target test count after Phase 7: **82 + 4 = 86** (no new
integration; 3 unit + 1 compile-time assertion).

### Phase 8 — Polish (detailed)

#### Clippy + dead-code sweep

Run `cargo clippy --workspace --all-targets -- -D warnings`
and resolve every `#[expect(dead_code)]` and
`#[expect(unused_variables)]` left over from Phases 1–7. The
only `#[expect(...)]` that should remain is the one on
`Default::default()` for the Linux-only resources on
non-Linux builds.

#### Runtime::about_to_wait line count

The 90-line target is met by:

* Moving the `PendingActions` drain block (lines 307–355) to
  `dispatch_pending_actions_system` in `AppSet::Settings`
  (Phase 6, 6.4).
* Moving the `apply_emotions` block (lines 357–363) to
  `apply_emotions_system` (Phase 7, 7.4).
* Moving the `update_char_window_cursor_and_hittest` call
  (line 432) and the `last_raycast_hit` write (line 461) to
  `update_cursor_state_system` +
  `raycast_bone_overlay_system` (Phase 7, 7.2).
* Moving the `cw.reconfigure` / `cw.window.request_redraw` /
  `drag::tick` block (lines 549–575) to a
  `handle_pointer_moved_system` in `AppSet::Input` (Phase 7,
  7.2).

The post-Phase-7 body is 9 lines, matching the snippet above.

#### Integration tests for the new `SettingsActionEvent` consumer systems

For each of the 7 `system/ui_actions/` modules added in
Phase 6:

* `integration: prev_next_character` — start a fresh `App`,
  register `UiPlugin`, write 2
  `SettingsActionEvent(PrevCharacter)` and 1
  `SettingsActionEvent(NextCharacter)`, run `app.update()`,
  assert the active character index moves `-2 + 1 = -1`
  (wraps).
* Same shape for the other 6 modules.

Target test count after Phase 8: **86 + 7 = 93**.

### Phase 9 — Documentation (detailed)

* Sync `docs/architecture/ene-desktop-ecs-migration.md` (this
  file) with the final `apps/ene-desktop/src/` layout — drop
  the `// NEW` markers from the Phase 6 plugin additions,
  update the `File Layout` block to include
  `plugin/platform_plugin.rs`, `plugin/tray_plugin.rs`,
  `plugin/ai_plugin.rs`, `system/ui_actions/`, and remove
  `resource/pending_actions.rs`.
* Translate to
  `docs/ja/architecture/ene-desktop-ecs-migration.md` (this
  file is the source of truth).
* Update `docs/applications/desktop.md` and
  `docs/ja/applications/desktop.md` to reflect the
  bevy-based architecture: replace the "Per-frame actions are
  dispatched in `Runtime::about_to_wait`" section with "All
  per-frame logic lives in `apps/ene-desktop/src/system/`;
  the runtime is a thin winit + GPU shim".
* Update the verification table at the top of this file:
  tests 75 → 93, lines `Runtime::about_to_wait` 90 → 9.

### Open questions (non-blocking; flag if any surprise you)

1. The pump-handle lifetime story in Phase 7's
   `NonSend<wgpu::Device>` and the `MaskReadbackWorker`
   (which holds its own `Arc<wgpu::Device>`) — this is a
   **clone**, not a borrow; the worker stays valid for the
   process lifetime because the device lives in the process.
   If you want the worker to be a bevy `NonSend` resource
   too, that is a small follow-up.
2. The `AppEvent::SessionSplit` and `AppEvent::StatusChanged`
   arms in `pump_events` (`ai_bridge.rs:186, 195`) are
   silently dropped today. The Phase 6 plan keeps that
   behaviour. If you want to surface them as `Message`s, add
   2 lines to the `MessageWriter` list in
   `pump_legacy_events`.
3. The Windows-only `register_character_colliders` path in
   `Runtime::resumed` (lines 200–213) still mutates
   `hecs`-free state; it can move into a `Startup` system in
   `CharacterPlugin` in Phase 7 with no functional change.
   Flagging in case you want it pulled forward into Phase 6.
