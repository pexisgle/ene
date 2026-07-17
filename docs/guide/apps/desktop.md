# Desktop Application (`ene-desktop`)

VRM character rendering with always-on-top overlay, winit + `wgpu`
+ `bevy_ecs 0.19` + `bevy_app 0.19`. The per-frame logic is owned
by a `bevy_app::App`; the winit `Runtime` is a thin driver that
calls `app.update()` every frame.

## Startup

Boot orchestration lives in `apps/ene-desktop/src/startup.rs`
(four phases — see [Startup Flow](../../reference/architecture/startup.md)).

```bash
cargo run -p ene-desktop
# Specify VRM:
cargo run -p ene-desktop -- /path/to/character.vrm
# VRM + VRMA animation:
cargo run -p ene-desktop -- /path/to/character.vrm /path/to/animation.vrma
```

## Architecture

The winit `Runtime` (`apps/ene-desktop/src/runtime.rs`) is an
`ApplicationHandler` that owns three winit windows (character,
dedicated chat, and settings) and their `wgpu::Surface`s. On every
`about_to_wait` it runs the full bevy schedule:

```rust
fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
    self.sync_runtime_to_bevy();
    self.state.app.update();
    if self.handle_exit(event_loop) { return; }
    self.run_debug_pipeline();
    self.render_per_frame(event_loop);
    self.set_frame_deadline(event_loop);
}
```

`app.update()` runs `First` / `PreUpdate` / `Update` / `PostUpdate`
/ `Last` (and `Startup` on the first call). After the schedule
returns the runtime:

* checks the `ExitRequested` resource and exits the loop if set;
* runs the per-frame cursor / hit-test pipeline (Linux: input
  region + click-through, Windows: `set_cursor_hittest`). The
  cursor source of truth is `device_query`, read in
  `update_char_window_cursor_and_hittest`; the bevy-side
  `update_cursor_state_system` is a no-op kept as a slot for
  a future `PointerMoved`-based migration;
* acquires + encodes + presents the character frame, the chat
  window, and the egui settings frame;
* schedules the next winit wake-up via
  `set_control_flow(WaitUntil(...))` based on
  `settings.graphics.target_fps`.

The bevy `App` is configured by `DesktopPlugins` in
`apps/ene-desktop/src/app.rs`:

| Plugin | Source | Role |
|--------|--------|------|
| `CorePlugin` | `app.rs` | `FrameState`, `ExitRequested`, `TokioHandle`, `EventChannels` (legacy bridge), the 13 `Message` types. |
| `CharacterPlugin` | `plugin/character_plugin.rs` | Spawns the `CharacterBundle` entity in `Startup`. |
| `PhysicsPlugin` | `plugin/physics_plugin.rs` | `attach_bone_colliders_system` in `Startup`; `step_physics_system` in `Update`. |
| `UiPlugin` | `plugin/ui_plugin.rs` | Spawns the `SettingsUiBundle` entity in `Startup`. |
| `PlatformPlugin` | `plugin/platform_plugin.rs` | Per-frame cursor state, input-region refresh, click-through. |
| `TrayPlugin` | `plugin/tray_plugin.rs` | Linux-only `tick_gtk_system` in `Last` (drain-only, no actual icon logic). |
| `ChatPlugin` | `plugin/chat_plugin.rs` | Spawns the `ChatUiBundle` entity in `Startup`. |
| `AiPlugin` | `plugin/ai_plugin.rs` | Adds the `system::ui_consumers` systems in `Update`. |

The schedule (`apps/ene-desktop/src/schedule.rs`) has six sets:

* `EventDispatch` (in `First`) — `pump_legacy_events` drains the
  legacy `AppEvent` receiver into typed bevy `Message`s.
* `Input` — `should_render_debug_system` (drag / debug-FPS
  gate). `update_cursor_state_system` is also in `Input` but
  is intentionally a no-op slot reserved for a future
  `PointerMoved`-based cursor source.
* `Settings` — `apply_linux_click_through_system`,
  `refresh_input_region_system`, `open_settings_system`,
  `apply_ai_text_deltas_system`, `apply_ai_permission_system`,
  `apply_ai_user_input_system`, `apply_emotions_system`,
  `apply_settings_action_system`.
* `Animation` — `step_physics_system`.
* `Render` / `Present` — placeholder sets; the actual GPU
  submission runs in `Runtime::render_per_frame` because
  `CharacterRenderer` is `!Send + !Sync`.

## AI Bridge

The desktop app uses the upstream `ene-runtime` actor (`EneHandle` /
`EneEvent`) through a thin `AiBridge` shim
(`apps/ene-desktop/src/ai_bridge.rs`). The bridge:

1. Opens a ready `EneHandle` via `EneHandle::open` on the current tokio runtime and
   subscribes to its broadcast `EneEvent` stream.
2. Spawns a background drain task that maps `EneEvent` →
   `AppEvent` (`AiStreamUpdate`, `PerformanceCue` / `EmoteToken`,
   `StatusChanged`, etc.) and pushes them into the cross-subsystem
   `AppEventSender`.
3. Owns a `processing: Arc<AtomicBool>` flag, set on
   `run` and cleared on `Terminal`.

User input flows back through `AiBridge::run` /
`AiBridge::cancel` (turn-scoped; `cancel` takes the active `TurnId`).

`EventChannels` (a bevy `Resource`) holds the receiver half of
the `AppEvent` bus. `system::event_pump::pump_legacy_events`
drains it in `First` / `EventDispatch` and writes the typed
`Message`s (`AiTextDelta`, `AiPermissionRequested`,
`AiUserInputRequested`, `AiStreamFinished`, `EmoteToken`, …)
that the per-frame `system::ui_consumers` systems then read.

```
tokio EneActor (ene-runtime)
  → EneEvent (broadcast)
    → AiBridge background task
      → AppEvent (mpsc)
        → EventChannels.rx
          → pump_legacy_events (First/EventDispatch)
            → Messages<AiTextDelta> / Messages<AiPermissionRequested> / …
              → apply_ai_text_deltas_system / apply_ai_permission_system / … (Update)
                → ChatStateComponent / UiStateComponent / CharacterSettings
```

### Chat window (#109)

User conversation lives in a dedicated egui window (`400 × 600`,
bottom-right by default), not in the settings AI tab.

| Control | Action |
|---------|--------|
| F2 | Toggle chat window |
| Tray → Chat | Show chat window |
| Enter | Send message (Shift+Enter inserts newline) |

Streaming text, permission prompts, and user-input dialogs are
handled on the chat window. History is reconciled from
`EneStateSnapshot.history` when the window opens and after each
completed turn.

### Message types

The messages registered by `CorePlugin` include:

| Message | Source | Consumer |
|---------|--------|----------|
| `AiTextDelta` | `pump_legacy_events` | `apply_ai_text_deltas_system` (chat entity) |
| `AiStreamFinished` | `pump_legacy_events` | `apply_ai_stream_finished_system` |
| `AiPermissionRequested` | `pump_legacy_events` | `apply_ai_permission_system` (opens chat) |
| `AiUserInputRequested` | `pump_legacy_events` | `apply_ai_user_input_system` (opens chat) |
| `EmoteToken` | `pump_legacy_events` | `apply_emote_tokens_system` |
| `OpenChat` | `pump_legacy_events` (tray) | `open_chat_system` |
| `PointerMoved` | `pump_window_events` | `update_cursor_state_system` (no-op; `device_query` is the cursor source of truth) |
| `PointerButton` | `pump_window_events` | (drag / future systems) |
| `KeyboardKey` | `pump_window_events` | (settings hotkey future) |
| `WindowResized` | `pump_window_events` | (resize handler) |
| `WindowCloseRequested` | `pump_window_events` | (exits the loop) |
| `OpenSettings` | `system::ui_dispatcher` | `open_settings_system` |
| `SettingsActionEvent` | `system::ui_dispatcher` | `apply_settings_action_system` (drain-only placeholder) |
| `TickGtk` | `pump_legacy_events` (Linux) | `tray_tick::tick_gtk_system` (drain-only) |

## Window Properties

| Property | Value |
|----------|-------|
| Character size | driven by `settings.graphics.character_size` |
| Chat size | 400 × 600 (dedicated chat window) |
| UI size | 460 × 620 (settings window) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |
| Hit test | Transparent areas are click-through (Linux: Wayland `set_input_region` + X11 `shape::rectangles`; Windows: `WS_EX_TRANSPARENT`) |

## Platform Support

| Feature | Linux (X11) | Linux (Wayland) | Windows |
|---------|:---:|:---:|:---:|
| VRM rendering | Yes (wgpu) | Yes (wgpu) | Yes (wgpu) |
| Always-on-top | Yes | Yes (layer shell) | Yes |
| System tray | Yes (gtk) | Yes (gtk) | Yes |
| Click-through | Yes (`shape` ext) | Yes (`set_input_region`) | Yes (`WS_EX_TRANSPARENT`) |
| Drag movement | Yes | Yes | Yes |
| Screenshot | Yes | Via portal | Yes |

## File layout

Plugin ordering and the ECS resource layout live under `apps/ene-desktop/src/plugin/` and `apps/ene-desktop/src/resource/`. The render path stays outside bevy systems because `CharacterRenderer` and wgpu types are `!Send + !Sync` — see [Startup Flow](../../reference/architecture/startup.md).
