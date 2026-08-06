# Desktop user guide

`ene-desktop` is the GUI application: a 3D VRM avatar, a chat pane, voice
input/output, an overlay/caption system, a system tray, and full settings
UI.

```sh
cargo run -p ene-desktop [vrm-path] [vrma-path]
```

The two positional arguments override the character's VRM model and a
default VRMA motion clip.

## Main window

- **3D avatar** — the loaded VRM character with expressions, motions,
  look-at tracking, spring bones, and scene physics (you can drag the
  model).
- **Chat pane** — conversation with the character; permission prompts and
  tool questions appear inline.
- **Caption overlay** — character speech is captioned on screen
  (`desktop.caption_enabled`).
- **Spotlight** — a global overlay for quick access (`desktop.spotlight_enabled`).

## Voice

With the `voice` feature (default), the app captures the microphone
(`desktop.mic_device`), transcribes with the configured STT provider,
streams replies through TTS, and drives the avatar's lip-sync from the
audio. Settings → Voice selects providers, models, voices, and speed.

## System tray & hotkeys

- The tray menu shows character status and lets you open the chat/settings
  or quit.
- A global hotkey opens the chat/spotlight overlay.

## Settings pages

| Page | What it edits |
|---|---|
| AI | Providers, task models, retry/fallback, TTS/STT/VAD |
| Character | Default expression/motion, look-at strength, position, scale |
| Character editor | Edit the active card's fields (see [Character editor](../guides/character-editor.md)) |
| Memory | Browse/search/pending/commitments tabs |
| Memory ledger | Full typed-memory review: edit, pin, approve, reject |
| Permissions | Standing tool-permission grants, revoke/reset |
| Connectors | External-service accounts (see [Connectors](../concepts/connectors.md)) |
| Sessions | List, archive, export sessions |
| Voice | STT/TTS/VAD selection and voice tuning |
| Graphics | Quality preset |
| Features | Feature toggles (spotlight, captions, proactive, …) |
| Accessibility | UI accessibility options |
| Debug | Diagnostics and pipeline detail |

## Platform notes

- **Linux** (X11/Wayland) is the supported platform; the app uses
  layer-shell overlays on Wayland where available.
- **Windows** is cross-compiled from Linux (mingw toolchain in the flake);
  there is no native Windows dev shell.
- **Headless CI** runs with software Vulkan (lavapipe):
  `DISPLAY=:1 WGPU_BACKEND=vulkan`.
- Building without audio: `cargo build -p ene-desktop --no-default-features`
  (voice modules become inert stubs).

## Data locations

Settings, characters, models, and the database live in the assets/app-data
directory — see [Configuration](../configuration.md).
