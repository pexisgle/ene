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
to 520 × 560. At 720 pixels or wider, its sidebar groups pages into two
categories — **Settings** (what a user tunes day to day) and **Management**
(data, accounts, and diagnostics). The search field at the top of the sidebar
replaces the category list with ranked page, section, and *plugin* results;
choosing a section opens its page, scrolls to the matching card, and
highlights it, and an active search reveals hidden advanced fields. Below 720
pixels, a grouped page picker and the same search UI replace the sidebar, and
setting controls wrap below their labels instead of requiring horizontal
scrolling. The window is keyboard-navigable (tab, arrows, Enter) and renders
correctly with long Japanese text.

| Category | Pages |
|---|---|
| Settings | Overview, General, Character & User, AI & Models, Voice & Audio, Behavior, Memory & Storage, Security & Downloads, Tools & Plugins, Advanced |
| Management | Character Card Editor, Memories, Sessions, Schedules, Permissions, Connectors, Diagnostics |

| Page | What it edits |
|---|---|
| Overview | Setup needs, health issues, restart-pending changes, and required credentials, each linking to the page that resolves it |
| General | Quality preset, UI language, Appearance theme, Spotlight, captions, accessibility |
| Character & User | Default expression/motion, look-at strength, position, scale, user name |
| Character Card Editor | Edit the active card's fields (see [Character editor](../guides/character-editor.md)) |
| AI & Models | Providers, task models, retry/fallback, provider health, and API keys (masked) |
| Voice & Audio | TTS, STT, microphone/VAD selection, and voice tuning |
| Behavior | Mind, emotion, and proactive speech behavior toggles |
| Memory & Storage | Memory enablement, approval workflow, retention limits, store integrity |
| Memories | Browse/search/pending/commitments tabs plus the full typed-memory ledger (edit, pin, approve, reject) |
| Schedules | Scheduled tool runs: create, enable, delete, run history |
| Tools & Plugins | Tool and provider lists with a per-tool detail view (actions, schema-driven config, profiles, security), MCP servers, runtime limits, detected-but-unconfigured binaries, validation |
| Advanced | Every remaining `settings.json` field, searchable and schema-driven |
| Sessions | List, search, import, export, and archive sessions |
| Permissions | Standing tool-permission grants, revoke/reset |
| Connectors | External-service accounts (see [Connectors](../concepts/connectors.md)) |
| Diagnostics | Runtime/AI/voice/plugin health, pipeline detail, and debug overlays |

Local model configuration is unified on plugin profiles: the AI page lists
profiles from `plugins.list.local-llm` / `llama-server` as model choices, and
`ai.local_models` is derived from those profiles at apply time instead of
being edited separately. The Schedules page validates cron / interval / JSON
inputs before submission, uses the host timezone by default, and can approve
or deny pending run confirmations directly.

### Draft apply pipeline

Pages never write to `settings.json` directly. Edits land on a draft that
tracks dirty paths, a monotonic revision, per-field schema validation, and
secret states; the window shows a **Pending changes** bar with Apply /
Discard. Applying runs asynchronously: the draft is secret-merged, validated
against the registered schemas *and* each dirty plugin's own `ValidateConfig`,
persisted atomically, and pushed to the runtime actor, which diffs it against
its live config and reports the actual impact — immediate, hot-reload,
plugin restart, or app restart — in a result banner. A stale draft is
rejected with a conflict, and a failed runtime apply rolls the persisted
config back. Stored secrets (API keys, plugin configs/profiles/credentials,
MCP auth headers) never live in UI state: the draft holds redacted
placeholders that are merged back from the store at apply time, and the
Tools & Plugins page receives host-redacted snapshots. List, search, and status
fetches across the settings and management pages run on the bridge runtime
and are polled asynchronously, so the render thread never blocks on IPC.

### Tools & Plugins

Tool activation and per-plugin settings live on one page. A General section
holds the plugin-system master switch, the Tool RAG toggle, and runtime
limits; Tools / Providers / MCP tabs then group the configured entries.
Tools and providers list each entry with health and an enable toggle;
opening one shows a kind-specific detail view with capabilities, manifest
facts, actions, config and profiles edited through the plugin's own JSON
Schema (typed controls with a raw-JSON fallback, unknown keys preserved),
and a Security section grouping sandbox, fs grants, DB quota, credentials,
and approval overrides. The effective-security summary (approval modes,
emergency stop, fs grants, sandbox, DB quota) is shown there too. Plugins
that advertise dynamic config answer the *Fetch options* button, and
*Validate config* runs the plugin's own validator. `x-ene-ui` metadata
(group/order/control/advanced/impact/options_path) and
`x-ene-profiles-schema` drive the forms.

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
