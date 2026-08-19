# Desktop user guide

`ene-desktop` is the product GUI. It starts `ene-core` when needed, draws
the character overlay with `ene-vrm`, keeps chat on the **surface** depth,
and opens a **separate detail window**.

```sh
cargo build -p ene-daemon -p ene-desktop
cargo run -p ene-desktop
```

The build step is required for the desktop's automatic local-core startup.
Native Windows development uses the same commands from PowerShell after
installing the stable MSVC Rust toolchain, Visual Studio C++ Build Tools, and
the Windows SDK.

| Window | Depth | Contents |
|---|---|---|
| Character overlay + chat | `surface` | Companion and speech. No inner / thinking / tool args |
| Detail (F4 / tray) | `detail` | Session log (including inner), thinking, tools, PAD, tasks |
| Settings | local + API | Apply writes the JSON layer (no API keys) to desktop `settings.json` and PATCHes core sections |

Desktop does not use a WebView. UI is egui; VRM is wgpu. The process talks to
the daemon only through `ene-api` (`client_id = desktop`). It does not link
`ene-daemon`, `ene-companion`, or the old runtime/mind/store crates.

Local `desktop.*` (graphics, theme, language, mic, captions, beat sync, core
lifetime) and the other applied sections (AI, mind, plugins) are persisted into
the shared `settings.json`. Debug builds use the source-tree `assets/` folder
as the data directory; release builds use the OS data directory and never read
repository `assets/`. API keys stay in the vault. Core reads and PATCHes that
same file. Attach to an already-running core with `ENE_API_URL` / `ENE_API_TOKEN`.

`ene-stage` remains an optional debug client for the same API.

Chat starts **unconfigured**. The surface prompts you to open the **AI** page.
Choices come from **installed provider plugins** (the host catalog), not a
hardcoded vendor list. Chat is a combo of catalog plugins that declare
`seam.llm` — including `provider.gguf` (This computer, local GGUF).

Embeddings are a separate optional picker of plugins that declare `seam.embed`
(or unset). A local GGUF embedding uses its own `llama-server` sidecar.
OpenAI-compatible and Anthropic model combos refresh through
`POST /api/v1/providers/models` (provider `list_models` IPC). Preset ids stay
as a fallback when the list is empty or the request fails. Local GGUF weights
install through the generic provider assets UI (`POST /api/v1/providers/assets/*`);
a custom file path remains available as an override.

Classifier and proactive routing live under **Advanced**. They use the same
plugin picker as chat. Leave a task unset to inherit the conversation model.
TTS/STT pickers list plugins that declare `seam.tts` / `seam.stt`.

Audio device relay and approval popups are the desktop's client-side jobs;
the daemon still owns policy and the live bus.
