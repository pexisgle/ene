# `ene-desktop` User Guide

`ene-desktop` is the GUI desktop application featuring real-time 3D VRM avatar rendering (`ene-vrm`), local voice synthesis/recognition (`ene-voice`), and live emotion/performance expressions.

---

## Launching Desktop

```bash
# Run Desktop application
cargo run -p ene-desktop
```

---

## Key Features

- **VRM 3D Avatar Window**: Renders VRM 1.0 models using `wgpu` hardware acceleration.
- **Lip-Sync & Expression Animation**: Receives `Performance` events from `ene-mind` to animate facial blendshapes (Happy, Angry, Surprised) and visemes (A, I, U, E, O).
- **Local Voice Pipeline**: Supports real-time microphone input via Silero VAD / Whisper STT and speech synthesis output.
- **Always-on-Top / Transparent Window**: Optional desktop overlay mode for a natural companion experience.
- **Character Card Editor**: Visual `CCv3` editing (identity, personality, scenario, greetings, memory instructions, lorebook, motion catalog) with schema/asset validation, atomic saves with backup, and discard confirmation. See the [Character Card Editor Guide](../guide/character-card-editor.md).

---

## Spotlight Quick Launcher (Alt+Space)

Press `Alt+Space` (`Option+Space` on macOS-style layouts) from any application to open the Spotlight command palette: a translucent, always-on-top input bar that works without focusing the avatar window.

- **Open Settings** — jumps to the AI settings page.
- **Open Chat** — opens the dedicated chat window.
- **Toggle Microphone** — starts/stops voice capture.
- **Toggle Caption Overlay** — shows/hides the floating caption window.
- **Free text** — when no command matches, pressing Enter sends the text to Ene through the same path as the chat input.

The palette is controlled from **Settings → Accessibility**: `Enable Spotlight (Alt+Space)` registers/unregisters the global shortcut (while it is off, Alt+Space passes through to the OS again), and `Enable floating caption overlay` controls whether the caption window can be shown.

### Platform limits

- **Wayland**: the `global-hotkey` crate has no Wayland backend, so the shortcut is registered as a global hotkey only on X11 and Windows. On Wayland the palette still opens with `Alt+Space` while Ene has focus (in-window fallback).
- **Windows**: `Alt+Space` is the OS window-menu shortcut; while the global registration is active, Ene consumes it system-wide.

---

## Floating Caption Overlay

The floating caption overlay is a separate, translucent subtitle window that renders the assistant's streamed speech in real time with a typewriter effect. Open it from Spotlight (`Toggle Caption Overlay`); Settings → Accessibility only enables the capability. Its position and pin state are remembered across restarts.

- **Drag** — grab the title bar to move it anywhere on screen; the position is remembered.
- **Pin / Unpin** — keeps the window above other applications (unavailable on Wayland, which has no window-level support).
- **Performance cues** — the latest cue name (e.g. `happy`) is shown as a small tag while speech is streaming.
- **Close** — the ✕ button hides the overlay; the caption feed keeps running until the next turn.
