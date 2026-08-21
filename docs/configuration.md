# Configuration

Ene loads settings as defaults → JSON → `ENE_` environment variables.
`__` separates nested keys (for example `ENE_CORE__SERVER__BIND`).

The daemon reads `settings.json` from the data directory, then overlays
`ENE_CORE__SERVER__*` and related env keys at boot. The repository file
`assets/settings.json` is a development sample, not the runtime file.
`ene-ctl` and `ene-stage` take `--url` / `--token` (or `ENE_API_URL` /
`ENE_API_TOKEN`) to reach an already-running core. When those env vars are
unset, `ene-stage` spawns `ene-core`.

The data directory is `ENE_DATA_DIR` when set. Otherwise debug builds use the
source-tree `assets/` folder for settings, databases, vault, and workspace,
and never write the OS data directory. Release builds use the OS data
directory and never read the repository `assets/` folder. Stage Apply and
core PATCH write the same `settings.json`. `GET /api/v1/settings` returns live
memory as `effective`; the on-disk file is `overlay` and does not replace live
AI, mind, or plugin bindings. API keys stay in the vault.

Add keys at the owning `define_config!` invocation. Schemas regenerate into
`assets/schema/` (gitignored — do not commit that directory).

| Section | Owner | Typical keys |
|---|---|---|
| `core` | `ene-kernel` | `server.bind`, `server.token_file`, `backup.*`, `clients.*` |
| `harness` | `ene-kernel` | `loop.max_steps_per_turn`, `retry.*`, `context.*`, `delegation.*`, `tool_output.soft_limit_bytes`, `tool_output.hard_limit_bytes` |
| `mind` | `ene-companion` | `inner.*`, `affect.*`, `recall.*`, `memory_approval.*`, `proactive.*` (`observation_interval_seconds` is the live tick interval; each open session is observed; `proactive.world_state.title_mode` and `ocr_hint`) |
| `characters` | `ene-companion` | `home_dir`, `import_v3` |
| `body` | `ene-body` | `render.*`, `autonomy.*` |
| `voice` | `ene-body` | `enabled`, `barge_in.*`, `input.routing` |
| `store` | `ene-session` | `sessions.db_path`, `sessions.idle_timeout_secs` |
| `approval` | `ene-plane` | `mode`, `popup.timeout_ms` |

Conversation, classifier, embedding, TTS, STT, approve, and job bind through
`ai.tasks.<task>`
(`plugin`, `model`, `model_path`, `base_url`, `voice`, `max_tokens`,
`supports_images`, `context_window`). Chat
starts unconfigured — set `ai.tasks.chat.plugin` to a `provider.*` id before
the first message. `supports_images` is opt-in and defaults to false: only a
configured binding with the flag set folds `ImageRef` tool results into
`LlmImage`; text-only or unknown providers keep `[image omitted]`. `approval.mode = ai_auto` uses `ai.tasks.approve` (chat
fallback) and pops up when that helper fails. Back-harness jobs use
`ai.tasks.job` (chat fallback, or the echo model when neither is bound) on a
lane independent of dialogue. There is no multi-provider failover: an empty
task row inherits chat (or stays disabled for TTS/STT/embedding). API keys stay in the vault (`ENE_AI__TASKS__<TASK>__API_KEY`
at boot; PATCH `/api/v1/settings` never writes them into JSON). Plugin ids are
`provider.*` names from the [plugin catalog](concepts/plugins-and-mcp.md)
(`GET /api/v1/settings` → `effective.providers`). Desktop does not keep a
parallel allowlist.

Local GGUF chat uses `provider.gguf` (`local: true`). Install weights and
`llama-server` from the AI or Engines pages (`provider.assets`). Optional
`model_path` / `server_path` override catalog installs. Cloud chat
uses any installed LLM plugin (API key in the vault).

Embeddings are optional on their own `ai.tasks.embedding` fiber: unset, local
GGUF (recommended Jina on `provider.gguf`), or a cloud plugin that declares
`seam.embed`. Empty classifier and proactive tasks inherit the chat binding.
Empty TTS and STT tasks stay disabled.

Observation privacy lives under `mind.proactive.world_state`: `title_mode` is
`app_only` (default), `redacted_title`, or `full_title`; `ocr_hint` is a local
opt-in slot with no bundled backend. The product GUI (Detail → Conversation)
shows the current send scope. Raw screenshots stay off session, memory, and
audit; only luma digest and text summary cross those boundaries.

Provider LLM calls (`ai.tasks.chat` / `job` / `classifier` / `approve`) retry
transient failures (`429` / `502` / `503` / timeout / overload) using
`harness.retry`. The last error becomes the turn / helper failure. The
effective context window is `min(provider advertised, ai.tasks.<task>.context_window)`
or 8192 when neither is set. `harness.context.response_reserve_tokens` (or
`max_tokens` when set) and `safety_margin_ratio` are subtracted; the dialogue
model then drops oldest non-system messages so the packed prompt fits.
Plugins do not yet advertise a window on hello — set `context_window` to cap
a large model.

| Key | Role |
|---|---|
| `harness.retry.max_attempts` | Total provider attempts, including the first. Default `3`. |
| `harness.retry.backoff_ms` | Sleep after each retryable failure. Default `[500, 2000, 8000]`. |
| `harness.context.response_reserve_tokens` | Completion reserve when `max_tokens` is unset. Default `4096`. |
| `harness.context.safety_margin_ratio` | Fraction of the window held back. Default `0.1`. |
| `harness.context.token_estimation` | `auto` (CJK-aware), `chars4`, or `cjk15`. |
| `ai.tasks.<task>.context_window` | Operator cap on the model window. Env: `ENE_AI__TASKS__<TASK>__CONTEXT_WINDOW`. |

Plugin launch is `plugins.profile` (`desktop`, `minimal`, or `headless`), not a
per-plugin enable map (`plugins.list` is gone). Related keys:

| Key | Role |
|---|---|
| `plugins.profile` | Launch tree. Default `desktop`. Env: `ENE_PLUGINS__PROFILE`. |
| `plugins.home_dir` | Install search path. Empty means `<data>/plugins`. Env: `ENE_PLUGINS__HOME_DIR`. |
| `plugins.policy.approval_mode` | Seeds `approval.mode` at boot (`ask_all`, `policy`, `ai_auto`, `auto`). Runtime truth stays `approval.mode`. |
| `plugins.policy.allow_unverified` | Allow a fiber whose digest does not match. Default `false`. |
| `plugins.ipc.max_frame_bytes` | IPC frame cap. Default `1048576`. Env: `ENE_PLUGINS__IPC__MAX_FRAME_BYTES`. |
| `plugins.ipc.bulk_threshold_bytes` | Payloads larger than this leave the MessagePack frame (`stream.open` / Unix `SCM_RIGHTS`). Default `65536`. Env: `ENE_PLUGINS__IPC__BULK_THRESHOLD_BYTES`. |

MCP servers are handwritten `mcp.json` rows, not settings keys. See
[Plugins & MCP](concepts/plugins-and-mcp.md).
