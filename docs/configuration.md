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
(`plugin`, `model`, `base_url`, `voice`, `max_tokens`). API keys are vault
secrets (`ENE_AI__TASKS__<TASK>__API_KEY` at boot; PATCH `/api/v1/settings`
never writes them into JSON). Plugin ids are `echo` or `provider.*` names
from the [plugin catalog](concepts/plugins-and-mcp.md).

Debug builds still resolve some bundled assets from the repository `assets/`
folder. Runtime data (`sessions.db`, `api.token`, `vault.bin`, workspace)
lives under the data directory, not next to the settings file.
