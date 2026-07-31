# Settings & Configuration Reference

Ene uses a layered configuration system powered by `figment`. Configuration settings are loaded in strict order of priority:

$$\text{Defaults} \longrightarrow \text{JSON Configuration File} \longrightarrow \text{Environment Variables (\texttt{ENE\_*})}$$

---

## 1. Environment Variable Precedence

Environment variables override any settings specified in default structs or JSON config files. Environment variables use the `ENE_` prefix, with double underscores (`__`) separating nested section keys:

```bash
# Example: Override default LLM chat model
export ENE_AI__TASKS__CHAT__MODEL="gpt-4o"

# Example: Specify SQLite database file path
export ENE_STORE__DB_PATH="/path/to/custom_memory.db"

# Example: Configure proactive speech interval (seconds)
export ENE_MIND__PROACTIVE__INTERVAL_SECONDS="300"
```

Environment-variable overrides are **transient**: they apply at runtime for the current process only and are never written back to `settings.json`. Saving the configuration persists only the JSON-layer values, so removing an `ENE_*` variable restores the underlying JSON/default value on the next launch (#326).

---

## 2. Configuration Sections

Public config sections are declared at their owning crates using `define_config!`.

### `ai.*` — LLM, Embeddings, & Voice Pipeline Settings

Contains provider definitions, task routing, retry/fallback rules, and voice (STT/TTS/VAD) settings:

```json
{
  "ai": {
    "providers": {
      "openai": {
        "kind": "openai",
        "api_key": "sk-...",
        "base_url": "https://api.openai.com/v1"
      }
    },
    "tasks": {
      "chat": {
        "provider": "openai",
        "model": "gpt-4o-mini"
      },
      "embedding": {
        "provider": "openai",
        "model": "text-embedding-3-small"
      }
    },
    "stt": { "enabled": true },
    "tts": { "enabled": true },
    "vad": { "enabled": true }
  }
}
```

### `store.*` — Database & Memory Persistence

Controls SQLite database persistence, integrity checks, and backup retention (#239):

```json
{
  "store": {
    "enabled": true,
    "backup_on_migrate": true,
    "max_backups": 5,
    "integrity_check_on_open": false
  }
}
```

### `mind.*` — Cognitive Engine & Emotion Parameters

Configures token context budget, hybrid memory recall, emotion decay, character compilation, and proactive speech policy (#103):

```json
{
  "mind": {
    "context": {
      "max_tokens": 4096,
      "recall_limit": 10
    },
    "emotion": {
      "enabled": true,
      "decay_half_life_minutes": 30.0
    },
    "proactive": {
      "enabled": true,
      "interval_seconds": 600
    },
    "memory": {
      "recall_min_score": 0.10,
      "recall_similarity_threshold": 0.35,
      "commitment_boost": 0.25
    }
  }
}
```

Memory recall uses a hybrid score `(relevance × quality + commitment_boost) × penalty`
(see `crates/ene-rag/src/scoring.rs`). A fresh, strongly relevant memory scores
near `1.0`; recent/lexical-only candidates land around `0.1–0.5`; unrelated
noise scores `0.0`. `recall_min_score` (default `0.10`) filters the final
ranking, `recall_similarity_threshold` (default `0.35`) gates the vector-gather
step, and `commitment_boost` (default `0.25`) lets active promises surface even
with zero query relevance.

`mind.emotion.classifier_language` (default `"en"`) selects the prompt-library
language used for the affect classifier and the cognitive output contract. The
user-facing LLM instruction strings are loaded at runtime from
`assets/lang/{lang}/prompts.json`; when that pack is absent the build falls back
to a compile-time embedded pack for the languages in `ene_config::SUPPORTED_LANGUAGES`
(`en`, `ja`), and otherwise to English. See
[Turns & Sessions](concepts/turn-and-session.md) §3 for details.

### `plugins.*` — IPC Plugins & MCP Server Connections

Manages out-of-process tool plugins and Model Context Protocol (MCP) servers:

```json
{
  "plugins": {
    "enabled": true,
    "list": {
      "app": { "enable": true },
      "browser": { "enable": true },
      "fs": { "enable": true },
      "utility": { "enable": true },
      "web": { "enable": true }
    },
    "max_concurrent": 8,
    "mcp_servers": [
      {
        "name": "filesystem",
        "command": "npx",
        "args": ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/allowed"]
      }
    ]
  }
}
```

HTTP MCP endpoints validate their URL before connecting (HTTPS-only by default;
loopback and cloud-metadata/link-local addresses refused). Set
`"mcp_allow_insecure_urls": true` inside `plugins` to permit plain-`http://` and
loopback URLs for local development; link-local addresses stay refused. See
[Plugins & MCP](concepts/plugins-and-mcp.md).

`max_concurrent` bounds the number of concurrent in-flight IPC requests **per
plugin connection**, across *all* request types (tool calls, pings,
`list_tools`, `chat_completion`, …) — not just tool calls. Requests beyond the
bound queue (bounded by their own timeout) rather than fanning out to the
plugin. Chat *streams* (`CreateChatStream`) are the exception: they bypass this
bound and are not counted against it.

### `tools.*` — Tool-Execution Runtime Behavior

Distinct from `plugins.*` (which manages the plugin *process*/IPC layer):
`tools.*` covers tool-invocation runtime knobs owned by `ene-runtime` and
`ene-rag`. `tools.rag` configures the Tool RAG selection pipeline
(`ene_rag::ToolRagConfig`); the fields shown below
(`ene_runtime::ToolRuntimeConfig`) cap how many background tasks the turn
actor keeps in flight at once and bound the deferred-tool poll budget. Once
a cap is reached, admission is rejected (fails fast) rather than queued
without bound — `CallTool`/`CancelDeferredTool` and `SearchTools` calls get
back an actionable "busy" error; the post-turn classifier, memory-writer,
and deferred-tool-poller admission points have no reply channel of their
own, so a rejection there only shows up as a `TaskRejected` diagnostic
event:

```json
{
  "tools": {
    "call_tool_cap": 64,
    "deferred_tool_cap": 32,
    "classifier_cap": 16,
    "memory_writer_cap": 16,
    "search_cap": 16,
    "deferred_max_polls": 600,
    "rag": {
      "enabled": true,
      "top_k": 12,
      "min_similarity": 0.20,
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "negative_threshold": 0.70
      }
    }
  }
}
```

Tool RAG scores each tool as a weighted average of its per-field embedding
similarities (`[-1, 1]`). `min_similarity` (default `0.20`) is the inclusion
floor for that average; `weights.negative_threshold` (default `0.70`) is the
gate at which a tool's negative-example embedding excludes it outright — a
tool that matches its own negative example this strongly is filtered, not
penalized.

### `desktop.*` — Desktop GUI & Graphics Parameters

Controls display language, graphics render parameters, and microphone input device:

```json
{
  "desktop": {
    "language": "en",
    "mic_device": null,
    "graphics": {
      "vsync": true
    }
  }
}
```

---

## 3. Character Card Format (`character.json`)

Ene loads character personalities and prompt templates via JSON character cards:

```json
{
  "name": "Alicia",
  "identity": "Cybernetic artificial intelligence companion living inside the computer.",
  "system_prompt": "You are Alicia, an energetic AI assistant. Answer directly and concisely.",
  "greeting": "Systems operational. What are we working on today?",
  "initial_affect": {
    "pleasure": 0.6,
    "arousal": 0.7,
    "dominance": 0.5
  }
}
```

---

## 4. Schema Generation

Settings schemas are declared at each owning crate via `define_config!`. Schemas are written once per process at application startup (CLI `init`, desktop first-launch, and the runtime open paths), not on every config load. Each schema file is written atomically (temp file + `fsync` + rename), so a crash can never leave a truncated schema behind.

When `settings.json` is saved, Ene automatically writes a relative `$schema` pointer (`./schema/settings.schema.json`) at the top of the file so editors provide completions and validation without the key being hand-written. An existing `$schema` value is preserved verbatim; the pointer is only filled in when it is absent. The user's hand-arranged top-level section order is likewise preserved across a save, and newly added sections append at the end.

> [!CAUTION]
> Never hand-edit or commit ignored schema files under `assets/schema/*`.
