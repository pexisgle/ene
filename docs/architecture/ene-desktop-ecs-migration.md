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
| 6 | `PlatformPlugin`, `TrayPlugin`, `AiPlugin` — Phase 6 work. The tray menu, the AI bridge pump, and the Linux Wayland / X11 input-region state migrate into bevy resources + systems. | ⏳ Pending |
| 7 | Render path integration — `acquire` / `encode` / `submit` / `present` are split into `Last`-stage systems. The `wgpu::Device` becomes a `NonSend` resource. `CharacterPlugin::finish` materialises the `VrmRenderer`. | ⏳ Pending |
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
* `cargo test -p ene-desktop` — 75 passed (72 + 3 new ECS plugin / bundle tests)
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
