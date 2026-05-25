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
            AiPlugin,           # AI streaming integration
            CharacterPlugin,    # Expression, animation, head tracking
            TrayPlugin,         # System tray
            SettingsUiPlugin,   # egui settings panel
            CharacterDragPlugin,# Click & drag movement
        ))
        .run()
```

### AI Integration (`AiPlugin`)

Bevy system chain:
1. `enqueue_ai_requests` — Bevy messages → internal queue
2. `process_embedding` — Background embedding computation
3. `start_next_ai_request` — Lazy memory init → card load → split task spawn → embedding → `run_ai_with_tools`
4. `poll_ai_worker` — Stream event consumption → display/sound/tool handling

Memory initialization is deferred to the first AI request.

### Window Properties

| Property | Value |
|----------|-------|
| Size | 560 × 980 (Windows) |
| Style | Windowed (Windows) / Borderless Fullscreen (macOS, Linux) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |

### Emotion Application

```
AiStreamEvent::SpecialToken → EmotionQueue → 4s hold + fade out
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
  │   └── AiRuntime::init(settings) → session + tool host
  ├── --tooltest → tooltest::run() → exit
  └── Normal mode:
      ├── build tool registry (ToolHostManager.start + MCP connect)
      └── repl::run(ctx) → interactive loop
```

### REPL Loop

1. Display prompt with `dialoguer::Input`
2. `/` commands handled by `commands::execute()`

| Command | Action |
|---------|--------|
| `/quit` | Exit |
| `/clear` | Clear history |
| `/prompt` | Show system prompt |
| `/card <path>` | Switch character card |
| `/config` | Show current settings |
| `/tools` | List enabled tools |
| `/history` | Show conversation history |
| `/undo` | Undo last file operation |
| `/tooltest [prompt]` | One-shot tool test |
| `/memory search <q>` | Search memory |
| `/memory list` | List stored summaries/facts |
| `/session split` | Manual session split |
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
