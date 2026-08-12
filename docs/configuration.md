# Configuration

Ene reads one global settings file plus one optional settings file per
character. Every value has a default, so an empty configuration still runs.

## Configuration files

| File | Purpose |
|---|---|
| `settings.json` | Global settings (AI providers, mind, store, tools, plugins, desktop). Lives in the assets directory. |
| `assets/characters/<name>/character.json` | The character card itself (see [Character cards](concepts/character-cards.md)). |
| `assets/characters/<name>/character_settings.json` | Per-character presentation settings (position, scale, default motion/expression, card language). |
| `assets/characters/<name>/character.<lang>.json` | Optional localized card diff (see [Localization](concepts/character-cards.md#localization)). |
| `assets/lang/<lang>/prompts.json`, `patterns.json` | Runtime prompt packs and forget-pattern packs (fall back to embedded copies). |

### Where the assets directory is

- Debug builds: the repository's `assets/` folder (so changes take effect
  immediately).
- Release builds: the OS application-data directory
  (`~/.local/share/ene` on Linux, `%APPDATA%\ene` on Windows).

The desktop app and CLI both accept `--config <path>` to load a different
`settings.json`; the CLI also accepts `--character <name>` and
`--lang <en|ja>`.

## Precedence

Settings are merged in this order (later wins):

1. Built-in defaults
2. `settings.json` on disk
3. `ENE_*` environment variables, with `__` separating nested keys

Examples:

```sh
ENE_AI__TASKS__CHAT__MODEL="openai/gpt-5.6-luna"
ENE_MIND__EMOTION__ENABLED="false"
ENE_TOOLS__LIST__WEB__ENABLE="false"
```

Environment variables can set any key that exists in `settings.json`
(including whole sections, when the value parses as JSON).

## Schema and validation

Every config section is defined by a `define_config!` invocation in the
owning crate (e.g. `ene-ai`, `ene-mind`, `ene-store`, `ene-plugin-host`,
`apps/ene-desktop`). At startup Ene regenerates JSON Schemas into
`assets/schema/` (`settings.schema.json`,
`character_settings.schema.json`, …), so editors get autocompletion and
validation. These schema files are generated artifacts and are not tracked
by Git.

`settings.json` carries a `version` field; older files are migrated forward
automatically on load (`ene-config` migrations). There is no migration
backward — the file is upgraded in place.

The desktop settings UI edits through a draft: changes are validated against
the registered schemas, persisted atomically, and pushed to the runtime,
which diffs them against its live config and reports the actual impact
(immediate / hot-reload / plugin-restart / app-restart). A failed runtime
apply rolls the persisted config back, and a draft based on a stale runtime
revision is rejected with a conflict so concurrent writers never overwrite
each other silently. `ai.local_models` is derived from the
`plugins.list.local-llm` / `llama-server` profiles at apply time — edit
profiles, never the derived map. Plugin configs (`plugins.list.<name>`
`config` / `profiles`) are edited with the plugin's own JSON Schema, with
`x-ene-ui` field metadata and `x-ene-profiles-schema` extensions; unknown
keys are preserved.

## Top-level keys

| Key | Type | Default | Meaning |
|---|---|---|---|
| `$schema` | string | — | Schema pointer for editor tooling (auto-filled on save). |
| `version` | number | 1 | Config schema version; migrated automatically. |
| `character` | string | `"Alicia"` | Character card name (or path) to load. |
| `user_name` | string | `"User"` | Display name used in prompts (`{{user}}`). |
| `runtime_rules` | string | built-in | Behavioural rules injected into every system prompt. |
| `user_persona` | object | — | Structured user persona; expands `{{user_persona}}`. |
| `ai` | object | see below | Providers, tasks, retry, fallback, TTS/STT/VAD. |
| `mind` | object | see below | Emotion, proactive, memory limits, topic boundaries, session. |
| `store` | object | `{ "enabled": true }` | Memory store toggles. |
| `tools` | object | see below | Tool enablement, MCP servers, tool RAG. |
| `plugins` | object | see below | Provider/tool plugin list with per-plugin settings. |
| `desktop` | object | see below | Desktop-only settings (graphics, language, captions, …). |

Unknown top-level keys are preserved across save (round-trip safe).

## `ai.*` — AI providers and tasks

```json
{
  "ai": {
    "providers": {
      "openrouter": {
        "kind": "openai_compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "api_key": { "source": "env", "env": "OPENROUTER_API_KEY", "inline": "" },
        "context_window": null
      }
    },
    "tasks": {
      "chat":       { "provider": "openrouter", "model": "openai/gpt-5.6-luna", "max_tokens": 8192, "supports_vision": true },
      "classifier": { "provider": "openrouter", "model": "openai/gpt-5.6-luna" },
      "embedding":  { "provider": "local",      "model": "jina-v5-small" },
      "proactive":  { "provider": "local",      "model": "gemma-4-e4b" }
    },
    "retry":     { "max_attempts": 3, "base_delay_ms": 500, "max_delay_ms": 30000, "timeout_ms": 120000 },
    "fallback":  { "enabled": false, "health_check_timeout_ms": 5000, "cache_ttl_ms": 60000, "max_history": 32 },
    "tts":       { "provider": "kokoro", "model": "kokoro-v1_0.onnx", "voice": "af_heart", "speed": 1.0, "language": "ja" },
    "stt":       { "provider": "whisper", "model": "", "language": "" },
    "vad":       { "provider": "none" }
  }
}
```

- **`ai.providers`** — named provider entries. `kind` is the provider kind
  (`openai`, `openai_compatible`, `anthropic`, `local`, …); `api_key`
  supports `source: "env"` (read from the named environment variable),
  `source: "inline"`, or `source: "file"`. Provider *kind* names are
  validated against the built-in set, with typo suggestions. For the
  broker-migrated `openai` plugin the key never reaches the plugin process:
  the host resolves it here and injects it into each API request
  (see [Sandbox, broker & approvals](concepts/sandbox-and-approvals.md)).
- **`ai.tasks`** — which provider+model serves which pipeline task:
  `chat` (conversation), `classifier` (LLM emotion classification),
  `embedding` (memory/tool vectors), `proactive` (proactive speech
  decisions). `dimensions` overrides embedding dimensions;
  `query_prefix` prefixes embedding queries.
- **`ai.retry`** — retry policy for transient provider errors.
- **`ai.fallback`** — optional failover to a second provider when health
  checks fail.
- **`ai.tts` / `ai.stt` / `ai.vad`** — voice pipeline provider selection
  (see [Voice & avatar](concepts/voice-and-avatar.md)). `model`/`voice`
  are provider-specific.
- **`ai.local_models`** — local GGUF model definitions (URL, context size,
  GPU layers, quantization, dimensions) used by the `local` provider.

## `mind.*` — cognitive engine

| Section | Notable keys | Meaning |
|---|---|---|
| `mind.language` | `"ja"` | Language for prompts/classifier defaults. |
| `mind.emotion` | `enabled`, `classifier_language` | PAD emotion engine and LLM classifier. |
| `mind.proactive` | `enabled`, `cooldown_seconds`, `interval_seconds`, `min_idle_seconds`, `sources`, `quiet_hours`, `paused` | Proactive speech gating (see [Proactive](reference/architecture/cognitive-runtime.md#proactive)). |
| `mind.memory_limits` | `commitment_active_match_limit` | Recall limits. |
| `mind.memory_approval` | `require_approval` | When true, extracted memories wait in a review queue before activation. |
| `mind.topic_boundary` | `enabled`, `boundary_threshold`, weights | Session split heuristics. |
| `mind.session` | `session_timeout_minutes` | Idle timeout that ends a session. |

## `store.*` — persistence

```json
{ "store": { "enabled": true } }
```

The memory store backs conversation history, typed memories, embeddings,
the commitment ledger, schedules, and audit logs in a single SQLite
database (`memory.db` under the assets directory). Disabling the store
disables memory features (chat still works without persistence).

## `tools.*` — tools and MCP

```json
{
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "homeassistant": { "enable": true, "base_url": "http://homeassistant.local:8123", "token": "" }
    },
    "mcp_servers": [],
    "rag": { "enabled": true }
  }
}
```

- `tools.list.<name>.enable` turns a built-in tool plugin on or off. New
  plugin configuration lives under `plugins.list.<name>`; host-owned secrets
  use its `credentials` map.
- `tools.mcp_servers` attaches external MCP servers (see
  [MCP servers guide](guides/tools/mcp-servers.md)).
- `tools.rag` configures embedding-based tool selection (the `tool` feature
  of `ene-rag`).

## `plugins.*` — plugin list

```json
{
  "plugins": {
    "list": {
      "llama-cpp": {
        "config": { "mmproj_url": "...", "acceleration": "auto" },
        "profiles": {
          "gemma-4-e2b": { "url": "...", "gpu_layers": "auto", "context_size": 16384 }
        }
      }
    }
  }
}
```

Each key names a plugin binary (`plugins.list.<name>`), and each plugin
declares its own configuration schema (`config`) plus optional named
`profiles` (model presets). The host passes the matching section to the
plugin over IPC at startup. Host-owned credentials belong in the entry's
`credentials` map and are injected by the network broker; they are never
included in `config`. For example, web search keys use
`plugins.list.web.credentials.exa_api_key` and
`plugins.list.web.credentials.tavily_api_key`. See [Plugins & MCP](concepts/plugins-and-mcp.md).

## `desktop.*` — desktop app

| Key | Default | Meaning |
|---|---|---|
| `desktop.graphics.quality` | `"medium"` | Render quality preset. |
| `desktop.language` | `"ja"` | UI language (desktop i18n files are `en-US` / `ja`). |
| `desktop.theme` | `"system"` | App-wide color theme: `system`, `light`, or `dark`. |
| `desktop.mic_device` | `null` | Microphone device id for voice input. |
| `desktop.spotlight_enabled` | `true` | Global spotlight overlay. |
| `desktop.caption_enabled` | `true` | Caption overlay for character speech. |
| `desktop.caption_position` / `caption_pinned` | `null` | Caption placement. |
| `desktop.beat_sync` | `{ "enabled": false, "device": null }` | Music beat-sync for avatar motion. |

`desktop.theme` defaults to `system`. On Linux, System reads and subscribes
to `org.freedesktop.appearance` `color-scheme` through the XDG settings
portal. On Windows, it uses winit's initial window theme and `ThemeChanged`
notifications. If the OS does not specify a scheme or it cannot be read, Ene
uses dark. Explicit `light` or `dark` settings override OS notifications and
also update supported native window decorations. The environment override is
`ENE_DESKTOP__THEME=system|light|dark`.

## Per-character settings (`character_settings.json`)

```json
{
  "character_position": [0, 0, 0],
  "model_scale": 1.0,
  "look_at_strength": 0.6,
  "default_motion": "",
  "default_expression": "neutral",
  "language": ""
}
```

- `character_position` / `model_scale` — placement of the VRM model in the
  desktop scene.
- `look_at_strength` — how strongly the avatar follows the cursor (0–1).
- `default_motion` / `default_expression` — names from the card's motion
  catalog / expressions.
- `language` — card language override (empty inherits the app language).

## Editing configuration at runtime

- **CLI REPL:** `/config set <dotted.key> <value>` (e.g.
  `/config set ai.tasks.chat.model openai/gpt-5.6-luna`). Values that parse
  as JSON are stored as JSON, otherwise as strings.
- **Desktop:** the Settings window edits the same sections through
  typed pages (AI, character, memory, permissions, connectors, voice, …).
- The CLI and desktop flags (`--config`, `--character`, `--lang`) override
  the file for one process invocation.

## Secrets

API keys and tokens are never written into logs or event streams: tool
arguments and free-text events pass through redaction before they are
emitted or persisted, and plugin config values are redacted at the host
boundary. Prefer `source: "env"` for API keys so secrets stay out of
`settings.json` entirely.
