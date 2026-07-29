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
    }
  }
}
```

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
      "top_k": 12
    }
  }
}
```

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

> [!CAUTION]
> Never hand-edit or commit ignored schema files under `assets/schema/*`.
