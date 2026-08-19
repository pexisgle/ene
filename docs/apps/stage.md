# Stage user guide

`ene-stage` is the product GUI. It starts `ene-core` when needed, draws the
character overlay with `ene-vrm`, keeps chat on the **surface** depth, and
opens a **separate detail window** for settings, memory, character management,
jobs, and internals.

```sh
cargo build -p ene-daemon -p ene-stage
cargo run -p ene-stage
```

The build step is required for stage's automatic local-core startup. Native
Windows development uses the same commands from PowerShell after installing
the stable MSVC Rust toolchain, Visual Studio C++ Build Tools, and the Windows
SDK.

| Window | Depth | Contents |
|---|---|---|
| Character overlay + chat | `surface` | Companion and speech. No inner / thinking / tool args |
| Detail (F1 / tray) | `detail` | Settings, memory, character, jobs/plugins, session log (including inner), thinking, tools, PAD |
| Spotlight (Alt+Space) | local | Quick actions: open detail tabs, toggle mic, quit |

Stage does not use a WebView. UI is egui; VRM is wgpu. The process talks to the
daemon only through `ene-api` (`client_id = stage`). It does not link
`ene-daemon`, `ene-companion`, or other daemon crates.

Local `desktop.*` keys (theme, language, mic, captions, beat sync, core
lifetime, overlay placement) and core sections (AI, plugins, …) are persisted
into the shared `settings.json`. Debug builds use the source-tree `assets/`
folder as the data directory; release builds use the OS data directory and
never read repository `assets/`. API keys stay in the vault. Attach to an
already-running core with `ENE_API_URL` / `ENE_API_TOKEN`.

Core lifetime defaults to `desktop.core_lifetime = app` (stop the child core
when stage exits). Set `detached` to leave the core running.

Chat starts **unconfigured**. Open the detail **Settings** tab and bind a chat
provider from installed plugins (`seam.llm`), including `provider.gguf` for
local GGUF. Provider asset install (GGUF weights / sidecar engines) lives under
the **Plugins** tab. TTS/STT pickers use plugins that declare `seam.tts` /
`seam.stt`.

Audio device relay, approval popups, tray, and OS notifications are stage's
client-side jobs; the daemon owns policy and the live bus.
