# Application Startup Flow

## Desktop (ene-desktop)

Bevy ECS-based application with a VRM character always-on-top overlay.

### Boot Sequence

```
main()
  ├── resources::ensure_resource_dirs()
  ├── read_cli_paths()                   # VRM/VRMA from CLI args or defaults
  ├── CharacterSettings::discover()      # Scan characters, load settings
  └── App::new()
        .insert_resource(settings)
        .add_plugins((
            DefaultPlugins.set(window_plugin()),  # Transparent always-on-top
            EguiPlugin,         # Settings UI
            VrmPlugin,          # VRM model loading
            VrmaPlugin,         # VRMA animation
            ScenePlugin,        # Camera, lights, frame limits
            EnePlugin,          # AI streaming via EneHandle actor
            CharacterPlugin,    # Expression, animation, head tracking
            TrayPlugin,         # System tray
            SettingsUiPlugin,   # egui settings panel
            CharacterDragPlugin,# Click & drag movement
        ))
        .run()
```

### AI Integration (`EnePlugin`)

The actor is initialized as a Bevy `Resource`:

```rust
#[derive(Resource)]
pub struct EneResource {
    pub handle: EneHandle,    // Actor handle — sends commands, receives events
    pub processing: bool,     // Whether an AI stream is active
}
```

Bevy system chain:
1. `enqueue_ai_requests` — Bevy `EneRequestEvent` messages → `handle.run()` (fire-and-forget)
2. `poll_ene_events` — `handle.try_recv()` in a loop → dispatches to `EneStreamEvent` messages

Events flow: `EneEvent` (broadcast) → `poll_ene_events` → `EneStreamEvent` (Bevy message) → UI/character systems.

**Important:** `poll_ene_events` uses `ene.handle.try_recv()` directly (not `clone()`) to avoid creating a new broadcast receiver every frame, which would cause event loss.

### Window Properties

| Property | Value |
|----------|-------|
| Size | 560 × 980 (Windows) |
| Style | Windowed (Windows) / Borderless Fullscreen (macOS, Linux) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |

### Emotion Application

```
EneEvent::SpecialToken → poll_ene_events → EneStreamEvent::SpecialToken
  → EmotionQueue → 4s hold + fade out
    → SetExpressions → VRM blendshape update
```

---

## CLI (ene-cli)

`#[tokio::main]` interactive REPL.

### Boot Sequence

```
main()
  ├── clap: Args parse (--tooltest flag)
  ├── config::init()
  │   ├── ensure_resource_dirs()
  │   ├── Load settings.json
  │   └── EneHandle::new() → spawns actor
  ├── --tooltest → tooltest::run() → exit
  └── Normal mode:
      ├── AppContext { handle: EneHandle }
      └── repl::run(ctx) → interactive loop
```

### REPL Loop

1. Display prompt with `dialoguer::Input`
2. `/` commands handled by `commands::execute()`
3. Regular input: `handle.run()` + `process_stream()` to display events

**Event subscription pattern:**
```rust
let mut rx = ctx.handle.subscribe();  // Get receiver before sending command
ctx.handle.run(&input);               // Send Run command
stream::process_stream(&mut rx, &ctx.handle).await;  // Process events
```

This ensures no events are lost between the `run()` call and the first `recv()`.

### REPL Commands

| Command | Action |
|---------|--------|
| `/quit` | Exit |
| `/clear` | Clear history |
| `/prompt` | Show system prompt |
| `/card <path>` | Switch character card (async load) |
| `/config` | Show current settings |
| `/tools` | List enabled tools |
| `/history` | Show conversation history |
| `/undo` | Undo last file operation |
| `/tooltest [prompt]` | One-shot tool test |
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
| `SpecialToken(emo)` | `[Emotion: name]` in magenta |
| `ToolCallStart` | `[Tool Calling: name(args)]` in cyan |
| `ToolCallResult` | `[Tool Result: ...]` in green |
| `SessionSplit` | Reason + summary in yellow |
| `Error` | Red text |
