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

The settings window opens at 900 × 700 logical pixels and can be resized down
to 520 × 560. At 720 pixels or wider, its sidebar groups all pages into four
categories. The search field at the top of the sidebar replaces the category
list with ranked page and section results; choosing a section opens its page,
scrolls to the matching card, and highlights it. Below 720 pixels, a grouped
page picker and the same search UI replace the sidebar, and setting controls
wrap below their labels instead of requiring horizontal scrolling.

| Category | Pages |
|---|---|
| Basics | Character, Character card, Display, Accessibility |
| AI & Voice | AI, Voice, Features |
| Data & Access | Memory, Memory ledger, Sessions, Permissions, Connectors |
| System | Debug |

| Page | What it edits |
|---|---|
| Character | Default expression/motion, look-at strength, position, scale |
| Character card | Edit the active card's fields (see [Character editor](../guides/character-editor.md)) |
| Display | Quality preset, UI language, and Appearance theme |
| Accessibility | Spotlight and caption overlays |
| AI | Providers, task models, retry/fallback, and provider health |
| Voice | TTS, STT, microphone/VAD selection, and voice tuning |
| Features | Voice, proactive, mind, and tool capability toggles |
| Memory | Browse/search/pending/commitments tabs |
| Memory ledger | Full typed-memory review: edit, pin, approve, reject |
| Sessions | List, search, import, export, and archive sessions |
| Permissions | Standing tool-permission grants, revoke/reset |
| Connectors | External-service accounts (see [Connectors](../concepts/connectors.md)) |
| Debug | Diagnostics and pipeline detail |

### Appearance

Display → Theme applies to the settings, chat, Spotlight, caption, and native
windows. **System** follows the OS color scheme, while **Light** and **Dark**
override OS changes immediately. See [`desktop.theme`](../configuration.md)
for platform behavior and configuration values.

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
