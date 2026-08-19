# Configuration

Ene loads settings as defaults → JSON → `ENE_` environment variables.
`__` separates nested keys (for example `ENE_CORE__SERVER__BIND`).

Add keys at the owning `define_config!` invocation (`ene-session`,
`ene-kernel`, `ene-companion`, `ene-body`, `ene-plane`, and others). Schemas
are regenerated at config init into `assets/schema/` (gitignored — do not
commit that directory).

The daemon reads `settings.json` from the data directory, then overlays
`ENE_CORE__SERVER__*` and related env keys at boot. `ene-ctl` and `ene-stage`
take `--url` / `--token` (or `ENE_API_URL` / `ENE_API_TOKEN`) to reach an
already-running core. `ene-desktop` does the same when those env vars are set;
otherwise it spawns `ene-core` and also persists a local `desktop.*` section
(graphics, theme, language, mic, overlays, core lifetime) in its own
`settings.json`.

Conversation, classifier, embedding, TTS, and STT bind through `ai.tasks.<task>`
(`plugin`, `model`, `model_path`, `base_url`, `voice`, `max_tokens`). Chat
starts unconfigured — set `ai.tasks.chat.plugin` to a `provider.*` id before
the first message. API keys are vault secrets (`ENE_AI__TASKS__<TASK>__API_KEY`
at boot; PATCH `/api/v1/settings` never writes them into JSON). Plugin ids are
`provider.*` names from the [plugin catalog](concepts/plugins-and-mcp.md)
(`GET /api/v1/settings` → `effective.providers`). Desktop does not keep a
parallel allowlist.

Local GGUF chat uses `provider.gguf` (`local: true`) with `model_path`.
When `server_path` is empty, the sidecar resolves `llama-server` from
configuration, CAS, `PATH`, or the bundled plugins directory. Cloud chat
uses any installed LLM plugin (API key in the vault).

Embeddings are optional on their own `ai.tasks.embedding` fiber: unset, local
GGUF (recommended Jina on `provider.gguf`), or a cloud plugin that declares
`seam.embed`. An empty classifier task reuses the chat binding. Empty TTS and
STT tasks stay disabled.

Plugin launch is `plugins.profile` (`desktop`, `minimal`, or `headless`), not a
per-plugin enable map. Related keys:

| Key | Role |
|---|---|
| `plugins.profile` | Launch tree. Default `desktop`. Env: `ENE_PLUGINS__PROFILE`. |
| `plugins.home_dir` | Install search path. Empty means `<data>/plugins`. Env: `ENE_PLUGINS__HOME_DIR`. |
| `plugins.policy.approval_mode` | Seeds `approval.mode` at boot (`ask_all`, `policy`, `ai_auto`, `auto`). Runtime truth stays `approval.mode`. |
| `plugins.policy.allow_unverified` | Allow a fiber whose digest does not match. Default `false`. |
| `plugins.ipc.max_frame_bytes` | IPC frame cap. Default `1048576`. Env: `ENE_PLUGINS__IPC__MAX_FRAME_BYTES`. |

MCP servers are handwritten `mcp.json` rows, not settings keys. See
[Plugins & MCP](concepts/plugins-and-mcp.md).

Debug builds still resolve some bundled assets from the repository `assets/`
folder. Runtime data (`sessions.db`, `api.token`, `vault.bin`, workspace)
lives under the data directory, not next to the settings file.
