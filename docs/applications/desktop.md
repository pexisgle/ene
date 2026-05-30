# Desktop Application (`ene-desktop`)

VRM character rendering with always-on-top overlay, powered by Bevy ECS.

## Startup

```bash
cargo run -p ene-desktop
# Specify VRM:
cargo run -p ene-desktop -- /path/to/character.vrm
# VRM + VRMA animation:
cargo run -p ene-desktop -- /path/to/character.vrm /path/to/animation.vrma
```

## Bevy Plugins

### Core Plugins

| Plugin | Role |
|--------|------|
| `DefaultPlugins` | Window, assets, input, rendering |
| `EguiPlugin` | Settings UI (immediate mode) |
| `VrmPlugin` | VRM 3D model loading |
| `VrmaPlugin` | VRMA animation loading and playback |

### Custom Plugins

| Plugin | Source | Role |
|--------|--------|------|
| `ScenePlugin` | `scene.rs` | Camera, lighting, environment, frame limits |
| `EnePlugin` | `ai_bridge.rs` | Actor-based AI streaming bridge (EneHandle + Bevy events) |
| `CharacterPlugin` | `character.rs` | Character spawn, expression blending, animation, head tracking |
| `TrayPlugin` | `tray.rs` | System tray icon and menu (Linux/Windows) |
| `SettingsUiPlugin` | `settings_ui/` | egui-based settings panel (AI, character, graphics) |
| `CharacterDragPlugin` | `character_drag/` | Click-and-drag window movement, transparent hit testing |

## AI Bridge (`EnePlugin`)

Connects Bevy's synchronous ECS world with the asynchronous actor-based `ene-core`:

```
Bevy ECS (synchronous)
  → EneHandle::run() (fire-and-forget via mpsc)
    → EneActor (background tokio task)
      → EneEvent (broadcast channel)
        → poll_ene_events → EneStreamEvent (Bevy message)
          → UI / character systems
```

### Resources

```rust
#[derive(Resource)]
pub struct EneResource {
    pub handle: EneHandle,    // Actor handle
    pub receiver: EneEventReceiver,  // Broadcast receiver for events
    pub processing: bool,     // Whether AI is streaming
}
```

### System Chain

1. `enqueue_ai_requests` — Receives `EneRequestEvent` → calls `handle.run()`
2. `poll_ene_events` — Calls `handle.try_recv()` in a loop → dispatches to `EneStreamEvent`

**Key design:** Uses `handle.try_recv()` directly (not `handle.clone()`) to avoid creating a new broadcast receiver every frame. Cloning would cause event loss because each new receiver only sees events from the point of subscription.

### Events

```rust
pub enum EneStreamEvent {
    TextDelta(String),
    SpecialToken(String),
    ToolCallStart { name: String, arguments: String },
    ToolCallResult { name: String, result: String },
    PermissionRequired { request_id, action, target, description },
    TaskProgress { task_id, step, total_steps, description },
    Finished,
    Error(String),
}
```

## VRM Character Pipeline

### Expression System

```
EneEvent::SpecialToken
  → poll_ene_events → EneStreamEvent::SpecialToken
  → EmotionQueue (enqueue)
  → process_emotion_queue (4s hold → fade-out)
  → SetExpressions trigger
  → VRM blendshape value update
```

### Animation Playback

VRMA files provide pre-made animations. `CharacterPlugin` manages playback state between idle, talking, and emotion-driven animations.

### Head Tracking

`CharacterDragPlugin` enables the character to follow the mouse cursor position for an interactive "look at cursor" effect.

## Window Properties

| Property | Value |
|----------|-------|
| Size | 560 × 980 (Windows) |
| Style | Windowed (Windows), Borderless Fullscreen (macOS, Linux) |
| Z-order | Always on top |
| Transparency | Composite alpha (OS-dependent) |
| Hit test | Transparent areas are click-through (Linux: Wayland layer shell) |

## Platform Support

| Feature | Linux (X11) | Linux (Wayland) | Windows |
|---------|:---:|:---:|:---:|
| VRM rendering | Yes (Bevy) | Yes (Bevy) | Yes (Bevy) |
| Always-on-top | Yes | Yes (layer shell) | Yes |
| System tray | Yes (gtk) | Yes (gtk) | Yes |
| Click-through | Yes | Yes (input region) | Yes |
| Drag movement | Yes | Yes (gtk overlay) | Yes |
| Screenshot | Yes | Via portal | Yes |
