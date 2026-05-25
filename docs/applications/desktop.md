# Desktop Application (`ene-desktop`)

Bevy ECS-based GUI app with VRM character rendering and always-on-top overlay.

## Launch

```bash
cargo run -p ene-desktop
# With specific VRM:
cargo run -p ene-desktop -- /path/to/character.vrm
# With VRM + VRMA animation:
cargo run -p ene-desktop -- /path/to/character.vrm /path/to/animation.vrma
```

## Bevy Plugins

### Core Plugins

| Plugin | Role |
|--------|------|
| `DefaultPlugins` | Window, assets, input, rendering |
| `EguiPlugin` | Immediate-mode GUI for settings |
| `VrmPlugin` | VRM 3D model loading |
| `VrmaPlugin` | VRMA animation loading and playback |

### Custom Plugins

| Plugin | Source | Role |
|--------|--------|------|
| `ScenePlugin` | `scene.rs` | Camera, lighting, environment, frame limiting |
| `AiPlugin` | `ai_bridge.rs` | Async AI streaming bridge between Bevy ECS and Tokio |
| `CharacterPlugin` | `character.rs` | Character spawning, expression blending, animation, head tracking |
| `TrayPlugin` | `tray.rs` | System tray icon and menu (Linux/Windows) |
| `SettingsUiPlugin` | `settings_ui/` | egui-based settings panel (AI, character, graphics) |
| `CharacterDragPlugin` | `character_drag/` | Click-and-drag window movement, transparent hit-testing |

## AI Bridge (`AiPlugin`)

Connects Bevy's synchronous ECS world to the async `ene-core` streaming engine:

```
Bevy ECS (sync)
  → Tokio runtime (async)
    → run_ai_with_tools()
      → AiStreamEvent pipeline
        → Bevy events back to ECS
```

System chain:
1. `enqueue_ai_requests` — Bevy UI messages → internal queue
2. `process_embedding` — Deferred embedding work from previous frame
3. `start_next_ai_request` — Memory init (lazy), card load, split task, embedding, AI launch
4. `poll_ai_worker` — Poll stream for events, dispatch to display/sound/tool systems

## VRM Character Pipeline

### Expression System

```
AiStreamEvent::SpecialToken
  → EmotionQueue (enqueued)
  → process_emotion_queue (4s hold → fade out)
  → SetExpressions trigger
  → VRM blendshape values updated
```

### Animation Playback

VRMA files provide pre-authored animations. The `CharacterPlugin` manages playback states and transitions between idle, talking, and emotion-driven animations.

### Head Tracking

The character can track the mouse cursor position via `CharacterDragPlugin`, creating an interactive "looking at cursor" effect.

## Window Properties

| Property | Value |
|----------|-------|
| Size | 560 × 980 (Windows) |
| Style | Windowed (Windows), Borderless Fullscreen (macOS, Linux) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |
| Hit-testing | Click-through for transparent areas (Linux: Wayland layer shell) |

## Platform Support

| Feature | Linux (X11) | Linux (Wayland) | Windows |
|---------|:---:|:---:|:---:|
| VRM rendering | Yes (Bevy) | Yes (Bevy) | Yes (Bevy) |
| Always on top | Yes | Yes (layer shell) | Yes |
| System tray | Yes (gtk) | Yes (gtk) | Yes |
| Click-through | Yes | Yes (input region) | Yes |
| Drag movement | Yes | Yes (gtk overlay) | Yes |
| Screenshots | Yes | Via portal | Yes |
