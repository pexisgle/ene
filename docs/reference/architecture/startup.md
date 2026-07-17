# Application Startup Flow

## Desktop (ene-desktop)

`ene-desktop` is a `winit` + `wgpu` + `egui` shell. The per-frame
logic is owned by a `bevy_app::App` whose scheduler is driven by the
winit event loop. **No Bevy plugins or Bevy renderer are used** —
only `bevy_ecs 0.19` / `bevy_app 0.19` for the scheduler, and
`ene-vrm` for VRM rendering.

### Startup Phases

Desktop startup is split into four explicit phases
(`apps/ene-desktop/src/startup.rs` orchestrates phases 1–2):

| Phase | When | Where | Work |
|-------|------|-------|------|
| **1 — First launch** | Once per install (release) | `startup::first_launch_setup` → `ene_config::ensure_resource_dirs` | Copy bundled default assets into the OS app-data directory |
| **2 — App launch** | Every process start (sync) | `startup::load_desktop_settings`, `startup::init_app_state`, eager `Runtime::new` Startup schedule | Load `settings.json` once (schema write once), discover characters, init GPU + ECS |
| **3 — Runtime warmup** | Every start (async) | `AiBridge` → `ene_runtime::bootstrap_runtime` | `reconfigure`, load character card, background tool embedding index (#108), CCv3 character-memory sync |
| **4 — Graphics ready** | After winit surface exists | `Runtime::resumed` | Tray, windows, VRM GPU init, platform click-through |

CLI uses the same Phase 3 path via `ene_runtime::bootstrap_runtime` in
`apps/ene-cli/src/config.rs` (loads config from disk inside bootstrap).

### Boot Sequence

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // … tracing + tokio runtime …

    let paths = startup::first_launch_setup()?;
    let gpu = pollster::block_on(gpu::GpuContext::new())?;
    let settings = startup::load_desktop_settings(&paths);
    let (app_state, event_tx) = startup::init_app_state(gpu, settings, &handle);

    let mut app = runtime::Runtime::new(app_state, event_tx);
    event_loop.run_app(&mut app)?;
    // … graceful tokio shutdown …
}
```

### `winit` → `bevy_app` Bridge

The winit `Runtime` (`apps/ene-desktop/src/runtime.rs`) implements
`ApplicationHandler` and owns two winit windows (character + settings)
plus their `wgpu::Surface`s. On every `about_to_wait` it runs the
full bevy schedule:

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
returns, the runtime:

- checks the `ExitRequested` resource and exits the loop if set;
- runs the per-frame cursor / hit-test pipeline (Linux: input
  region + click-through, Windows: `set_cursor_hittest`);
- acquires + encodes + presents the character frame and the
  egui settings frame;
- schedules the next winit wake-up via
  `set_control_flow(WaitUntil(...))` based on
  `settings.graphics.target_fps`.

### Bevy Plugins (`DesktopPlugins`)

The bevy `App` is configured in `apps/ene-desktop/src/app.rs`:

| Plugin | Source | Role |
|--------|--------|------|
| `CorePlugin` | `app.rs` | `FrameState`, `ExitRequested`, `TokioHandle`, `EventChannels` (legacy bridge), the 13 `Message` types. |
| `CharacterPlugin` | `plugin/character_plugin.rs` | Spawns the `CharacterBundle` entity in `Startup`. |
| `PhysicsPlugin` | `plugin/physics_plugin.rs` | `attach_bone_colliders_system` in `Startup`; `step_physics_system` in `Update`. |
| `UiPlugin` | `plugin/ui_plugin.rs` | Spawns the `SettingsUiBundle` entity in `Startup`. |
| `PlatformPlugin` | `plugin/platform_plugin.rs` | Per-frame cursor state, input-region refresh, click-through. |
| `TrayPlugin` | `plugin/tray_plugin.rs` | Linux-only `tick_gtk_system` in `Last` (drain-only). |
| `AiPlugin` | `plugin/ai_plugin.rs` | Adds the `system::ui_consumers` systems in `Update`. |

The schedule (`apps/ene-desktop/src/schedule.rs`) has six sets:

- `EventDispatch` (in `First`) — `pump_legacy_events` drains the
  legacy `AppEvent` receiver into typed bevy `Message`s.
- `Input` — `should_render_debug_system` (drag / debug-FPS gate).
  `update_cursor_state_system` is in `Input` but is intentionally
  a no-op slot reserved for a future `PointerMoved`-based cursor
  source.
- `Settings` — `apply_linux_click_through_system`,
  `refresh_input_region_system`, `open_settings_system`,
  `apply_ai_text_deltas_system`, `apply_ai_permission_system`,
  `apply_ai_user_input_system`, `apply_emotions_system`,
  `apply_settings_action_system`.
- `Animation` — `step_physics_system`.
- `Render` / `Present` — placeholder sets; the actual GPU
  submission runs in `Runtime::render_per_frame` because
  `CharacterRenderer` is `!Send + !Sync`.

### AI Integration (`AiBridge`)

`ene-desktop` consumes the upstream `ene-runtime` actor (`EneHandle` /
`EneEvent`) through a thin shim
(`apps/ene-desktop/src/ai_bridge.rs`). The bridge:

1. Opens a ready `EneHandle` via `EneHandle::open` on the current tokio runtime and
   subscribes to its broadcast `EneEvent` stream.
2. Spawns a background drain task that maps `EneEvent` →
   `AppEvent` (`AiStreamUpdate`, `PerformanceCue` / `EmoteToken`,
   `StatusChanged`, etc.) and pushes them into the cross-subsystem
   `AppEventSender`.
3. Spawns Phase 3 runtime warmup via
   [`ene_runtime::bootstrap_runtime`](../../../crates/ene-runtime/src/bootstrap.rs)
   using the config already loaded by `CharacterSettings::discover`
   (no second disk load, no duplicate schema write).
4. Owns a `processing: Arc<AtomicBool>` flag, set on
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
                → UiStateComponent / CharacterSettings
```

#### The 13 `Message` types registered by `CorePlugin`

| Message | Source | Consumer |
|---------|--------|----------|
| `AiTextDelta` | `pump_legacy_events` | `apply_ai_text_deltas_system` |
| `AiStreamFinished` | `pump_legacy_events` | (consumed by the AI page itself) |
| `AiPermissionRequested` | `pump_legacy_events` | `apply_ai_permission_system` |
| `AiUserInputRequested` | `pump_legacy_events` | `apply_ai_user_input_system` |
| `EmoteToken` | `pump_legacy_events` | `apply_emotions_system` |
| `PointerMoved` | `pump_window_events` | `update_cursor_state_system` (no-op; `device_query` is the cursor source of truth) |
| `PointerButton` | `pump_window_events` | (drag / future systems) |
| `KeyboardKey` | `pump_window_events` | (settings hotkey future) |
| `WindowResized` | `pump_window_events` | (resize handler) |
| `WindowCloseRequested` | `pump_window_events` | (exits the loop) |
| `OpenSettings` | `system::ui_dispatcher` | `open_settings_system` |
| `SettingsActionEvent` | `system::ui_dispatcher` | `apply_settings_action_system` (drain-only placeholder) |
| `TickGtk` | `pump_legacy_events` (Linux) | `tray_tick::tick_gtk_system` (drain-only) |

### Window Properties

| Property | Value |
|----------|-------|
| Character size | driven by `settings.graphics.character_size` |
| UI size | 460 × 620 (settings window) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |
| Hit test | Transparent areas are click-through (Linux: Wayland `set_input_region` + X11 `shape::rectangles`; Windows: `WS_EX_TRANSPARENT`) |

### Platform Support

| Feature | Linux (X11) | Linux (Wayland) | Windows |
|---------|:---:|:---:|:---:|
| VRM rendering | Yes (wgpu) | Yes (wgpu) | Yes (wgpu) |
| Always-on-top | Yes | Yes (layer shell) | Yes |
| System tray | Yes (gtk) | Yes (gtk) | Yes |
| Click-through | Yes (`shape` ext) | Yes (`set_input_region`) | Yes (`WS_EX_TRANSPARENT`) |
| Drag movement | Yes | Yes | Yes |
| Screenshot | Yes | Via portal | Yes |

### Emotion Application

```
EneEvent::Performance → AiBridge → AppEvent::PerformanceCue
  → pump_legacy_events → Message<EmoteToken>
    → apply_emotions_system (hold + fade out)
      → SetExpressions → VRM blendshape update (ene-vrm)
```

For more details see the full file layout in
[`docs/guide/apps/desktop.md`](../../guide/apps/desktop.md).

---

## CLI (ene-cli)

`#[tokio::main]` interactive REPL.

### Boot Sequence

```
main()
  ├── clap: Args parse
  ├── config::init()
  │   ├── ConfigStore::try_load → card
  │   └── EneHandle::open(config, card) → ready handle
  └── Normal mode:
      ├── AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
      └── repl::run(ctx) → interactive loop
```

### REPL Loop

1. Display prompt with `dialoguer::Input`
2. `/` commands handled by `commands::execute()` via `CliCommand` trait
3. Regular input: `handle.run()` → `TurnId` + `process_stream()` to display events

**Event subscription pattern:**
```rust
let mut rx = ctx.handle.subscribe();  // Get receiver before sending command
let turn = ctx.handle.run(&input)?;   // Returns TurnId (or Busy)
stream::process_stream(&mut rx, &ctx.handle, turn).await;  // Process events
```

This ensures no events are lost between the `run()` call and the first `recv()`.

### REPL Commands

| Command | Action |
|---------|--------|
| `/quit` | Exit |
| `/clear` | Note: history will be refreshed on next run (manual clear is a no-op) |
| `/prompt` | Show system prompt |
| `/card <path>` | Switch character card (async load) |
| `/config` | Show current settings |
| `/tool list` | List all registered tools |
| `/tool help <name>` | Show detailed help for a tool |
| `/tool call <name> <json>` | Call a tool directly |
| `/history` | Show conversation history |
| `/undo` | Placeholder (not yet supported with actor-based runtime) |
| `/memory search <q>` | Search memory |
| `/memory list` | List stored summaries/facts |
| `/session split` | Manual session split (via ManualSplit command) |
| `/session info` | Session diagnostics |
| `/session summaries` | Past summary list |
| `/help` | Help |

### Stream Display Formatting

| Event | Output Style |
|-------|-------------|
| `TextDelta` | stdout (flush) |
| `Performance` | `[Performance: name]` (cue display) |
| `ToolCallStart` | `[Tool Calling: name(args)]` in cyan |
| `ToolCallResult` | `[Tool Result: ...]` in green |
| `ContextCompressed` | Thin compression notice (optional) |
| `Terminal` / failed reason | Error text in red when `Failed` |
