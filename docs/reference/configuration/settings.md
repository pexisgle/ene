# Configuration

ene settings are centralized in `assets/settings.json` (or the OS user config directory on first launch). A `settings.schema.json` is auto-generated for editor validation.

Loading: `ene_config::load_full_config()` / `ConfigStore` resolves defaults, file, and environment variables.

**API v2 ownership:** persistence toggles live under `store` (`enabled` only in the public schema). Recall / write / decay / MMR / emotion internals / performance policy knobs use **code defaults** under `mind.*` — only `mind.emotion` and `mind.proactive` policy fields are user-facing. There is no top-level `memory.*` policy section and no `cognition.enabled` dual-pipeline switch — the mind path is the only streaming path.

## Top-Level Structure (`EneConfig`)

```rust
pub struct EneConfig {
    pub version: u32,           // Currently 2
    pub character: String,      // Character folder name or card path
    pub user_name: String,      // Default "User"
    pub extra: HashMap<String, serde_json::Value>, // Section map (ai, store, tools, mind, desktop, …)
}
```

`runtime_rules` (overlay-oriented behavioural instructions) is **not** part of the public settings schema. It is a compile-time constant (`DEFAULT_RUNTIME_RULES` in `ene-config`) injected into every system prompt.

### Character Resolution Rules

- **Prefer the folder name** (e.g. `"Alicia"`) over a full path — this matches how desktop discovers characters and keeps configs portable.
- Empty string `""` → `assets_dir/characters/Alicia/character.json` (backward compatibility).
- Name without a path separator → `assets_dir/characters/{name}/character.json`.
- Path with `/` or `\` → used as-is (absolute or relative card path).

### Migration (version 1 → 2)

**No automatic migration.** Rewrite `settings.json` by hand: set `"version": 2` and replace the top-level `provider` block with `ai` (`providers` + `tasks`). Old `provider` keys are ignored.

| Old (`provider`) | New (`ai`) |
|------------------|------------|
| `name`, `base_url`, `api_key` | `providers.<name>` with `"kind": "openai_compatible"` |
| `model`, `max_tokens` | `tasks.chat` |
| `embedding.*` | `tasks.embedding` (+ optional `query_prefix`) |
| `proactive.generation_model` | `tasks.proactive` (optional `TaskRef`; `null` = use chat) |
| `proactive.decision.*` | `providers.<name>` with `"kind": "local_gguf"` and `model_path`, referenced from `tasks.proactive` |

Still code-defaults only (not in the thin public UI surface, but some remain JSON-configurable): top-level `session`, `web_config`, `tools.max_rounds` / `timeout_ms`, `mind.context`, most of `mind.memory` / `mind.character`, extended `mind.proactive` / `mind.emotion` knobs, `store.db_path`, `runtime_rules`. **Configurable again:** `tools.rag`, filesystem sandbox limits under `tools.list.fs`.

## Complete Example

```json
{
  "version": 2,
  "character": "Alicia",
  "user_name": "User",
  "ai": {
    "providers": {
      "default": {
        "kind": "openai_compatible",
        "base_url": "",
        "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
      }
    },
    "tasks": {
      "chat": { "provider": "default", "model": "gpt-4o-mini", "max_tokens": 8192 },
      "embedding": { "provider": "default", "model": "text-embedding-3-small", "dimensions": 1536 },
      "classifier": null,
      "proactive": null
    }
  },
  "store": { "enabled": false },
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true, "allowed_directories": ["."], "writable_directories": ["."] },
      "web": { "enable": true, "tavily_api_key": "", "brave_api_key": "", "exa_api_key": "" },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": []
  },
  "mind": {
    "emotion": { "enabled": true },
    "proactive": {
      "enabled": false,
      "interval_seconds": 60,
      "min_idle_seconds": 120,
      "cooldown_seconds": 300
    }
  },
  "desktop": {
    "language": "en",
    "graphics": { "quality": "medium" }
  }
}
```

## Sections

### `ai` — Provider Registry and Task Routing

The `ai` section replaces the legacy `provider` block. Named providers are defined once; each cognitive workload (`chat`, `embedding`, `classifier`, `proactive`) points at a provider and optional model overrides.

```json
{
  "ai": {
    "providers": {
      "default": {
        "kind": "openai_compatible",
        "base_url": "",
        "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
      }
    },
    "tasks": {
      "chat": { "provider": "default", "model": "gpt-4o-mini", "max_tokens": 8192 },
      "embedding": { "provider": "default", "model": "text-embedding-3-small", "dimensions": 1536 },
      "classifier": null,
      "proactive": null
    }
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `providers` | object | Map of provider name → provider definition |
| `tasks.chat` | object | Main conversation model (required) |
| `tasks.embedding` | object | Embedding model (required) |
| `tasks.classifier` | object or `null` | Affect classifier; `null` → falls back to `tasks.chat` |
| `tasks.proactive` | object or `null` | Proactive generation routing; `null` → falls back to `tasks.chat` |

#### `ai.tasks` — Task Reference (`TaskRef`)

| Field | Type | Description |
|-------|------|-------------|
| `provider` | string | Key in `ai.providers` |
| `model` | string | Model name (required for `openai_compatible` chat/embedding) |
| `max_tokens` | int | Max completion tokens for chat workloads (`0` = omit from request). OpenRouter reserves credit collateral against this ceiling |
| `dimensions` | int | Expected embedding dimensions (cloud embedding) |
| `query_prefix` | string or null | Optional prefix prepended to embedding retrieval queries (e.g. `"Query: "`) |

#### `ai.providers` — Provider Kinds

Each provider is a tagged object with `"kind"`:

##### `openai_compatible`

Cloud chat, embedding, classifier, and cloud proactive decision via an OpenAI-compatible HTTP API.

```json
{
  "kind": "openai_compatible",
  "base_url": "https://api.openai.com/v1",
  "api_key": { "source": "env", "env": "OPENAI_API_KEY", "inline": "" }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `base_url` | string | `""` | API base URL. Empty → `OPENAI_BASE_URL` env var |
| `api_key` | object | (see below) | API key configuration |

###### `api_key`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `source` | string | `"env"` | `"inline"` or `"env"` |
| `inline` | string | `""` | API key when `source = "inline"` (use with caution) |
| `env` | string | `"OPENAI_API_KEY"` | Env var name when `source = "env"` |

##### `local_gguf`

Local GGUF via in-process llama-cpp-2. Used for **embedding** (Hub model name) and/or **proactive decision** (`model_path`).

```json
{
  "kind": "local_gguf",
  "model": "jina-embeddings-v5-text-small",
  "quantization": "F16",
  "model_path": "",
  "acceleration": "auto",
  "gpu_layers": "auto",
  "context_size": 2048
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"jina-embeddings-v5-text-small"` | Hub model name for embedding |
| `quantization` | string | `"F16"` | Quantization level (e.g. `"F16"`, `"Q4_K_M"`) |
| `model_path` | string | `""` | Filesystem path to decision GGUF. Empty = embedding-only / Hub download |
| `acceleration` | string | `"auto"` | `"auto"`, `"vulkan"`, `"cuda"`, or `"cpu"` |
| `gpu_layers` | string | `"auto"` | `"auto"` or an integer string for GPU layer offload |
| `context_size` | int | `2048` | Context size for the decision model |

**Routing rules:**

- `tasks.chat` and `tasks.classifier` must use `openai_compatible` providers.
- `tasks.embedding` may use either kind.
- `tasks.classifier: null` → classifier reuses `tasks.chat` provider and model.
- `tasks.proactive: null` → proactive **generation** reuses `tasks.chat`.
- Proactive **decision**: if `tasks.proactive` points at a `local_gguf` provider with a non-empty `model_path`, the in-process GGUF runs locally (load failure falls back to cloud when an OpenAI-compatible chat/proactive task exists). If `tasks.proactive` points at `openai_compatible`, that task's model is used for the cloud decision call; otherwise `tasks.chat` is used. See [Proactive Speech ADR](../architecture/proactive-speech.md).

GGUF weights are not bundled; paths are user-supplied. No external `llama-server` binary is required.

#### Multi-Provider Example

OpenRouter for chat + classifier, local embedding:

```json
{
  "ai": {
    "providers": {
      "openrouter": {
        "kind": "openai_compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "api_key": { "source": "env", "env": "OPENROUTER_API_KEY", "inline": "" }
      },
      "local_embed": {
        "kind": "local_gguf",
        "model": "jina-embeddings-v5-text-small",
        "quantization": "F16",
        "model_path": "",
        "acceleration": "auto",
        "gpu_layers": "auto",
        "context_size": 2048
      }
    },
    "tasks": {
      "chat": {
        "provider": "openrouter",
        "model": "xiaomi/mimo-v2.5",
        "max_tokens": 8192
      },
      "embedding": { "provider": "local_embed" },
      "classifier": {
        "provider": "openrouter",
        "model": "google/gemini-2.5-flash-lite"
      },
      "proactive": null
    }
  }
}
```

### `store` — Persistent SQLite-vec Store

```json
{
  "store": {
    "enabled": false
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable the persistence store |

The database path is resolved automatically (`assets/characters/{name}/memory.db`). It is not user-configurable in the public schema.

### `tools` — Tool Configuration

```json
{
  "tools": {
    "enabled": true,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": []
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable function calling for all tools |
| `list` | object | (built-in tools) | Per-tool enable map with optional flattened config |
| `mcp_servers` | array | `[]` | MCP servers list |

`max_rounds` and `timeout_ms` use **code defaults** and are not in the thin public UI schema. Tool RAG is configured under `tools.rag` (see below).

#### `tools.list` — Per-Tool Entries

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `<name>.enable` | bool | `true` | Enable/disable a specific tool |
| `<name>.*` | varies | — | Tool-specific fields flattened into the entry (no nested `config` object) |

##### `tools.list.fs` — Filesystem Sandbox

```json
{
  "fs": {
    "enable": true,
    "allowed_directories": ["."],
    "writable_directories": ["."],
    "blocked_commands": ["rm\\s+-rf\\s+/", "dd\\s+if=", "mkfs", "sudo\\s+", ":\\s*\\{\\s*\\|\\s*&\\s*;\\s*\\}"],
    "max_read_bytes": 51200,
    "max_write_bytes": 1048576,
    "shell_timeout_ms": 120000,
    "max_shell_output_bytes": 51200,
    "max_shell_output_lines": 2000
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `allowed_directories` | string[] | `["."]` | Directories allowed for read access |
| `writable_directories` | string[] | `["."]` | Directories allowed for write access |
| `blocked_commands` | string[] | (dangerous-command regexes) | Regex patterns for blocked shell commands |
| `max_read_bytes` | int | `51200` | Maximum bytes per read |
| `max_write_bytes` | int | `1048576` | Maximum bytes per write |
| `shell_timeout_ms` | int | `120000` | Shell command timeout |
| `max_shell_output_bytes` | int | `51200` | Maximum shell output bytes |
| `max_shell_output_lines` | int | `2000` | Maximum shell output lines |

##### `tools.rag` — Tool RAG Pipeline

```json
{
  "rag": {
    "enabled": true,
    "use_hyde": true,
    "use_rerank": true,
    "top_k": 12,
    "final_n": 6,
    "background_index_on_startup": true
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable Tool RAG (when tools are enabled, embedder is required if this is true) |
| `use_hyde` | bool | `true` | Hypothetical document embedding expansion |
| `use_rerank` | bool | `true` | Embedding-based candidate rerank |
| `top_k` | int | `12` | Pre-rerank candidate count |
| `final_n` | int | `6` | Final tools returned |
| `background_index_on_startup` | bool | `true` | Warm the index at startup |

##### `tools.list.web` — Web Search API Keys

```json
{
  "web": {
    "enable": true,
    "tavily_api_key": "",
    "brave_api_key": "",
    "exa_api_key": ""
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tavily_api_key` | string | `""` | Tavily Search API key |
| `brave_api_key` | string | `""` | Brave Search API key |
| `exa_api_key` | string | `""` | Exa Search API key |

#### `tools.mcp_servers` — Model Context Protocol Servers

```json
{
  "mcp_servers": [
    {
      "name": "my-server",
      "enabled": true,
      "transport": {
        "type": "stdio",
        "command": "/usr/bin/my-mcp-server",
        "args": ["--verbose"]
      }
    },
    {
      "name": "http-server",
      "enabled": true,
      "transport": {
        "type": "http",
        "url": "http://localhost:3000/mcp"
      }
    }
  ]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Server name (display and routing) |
| `enabled` | bool | Whether this MCP server is enabled |
| `transport` | object | Transport configuration (see below) |

**Transport types:**

| Type | Fields | Description |
|------|--------|-------------|
| `stdio` | `command`, `args` | Spawn a child process with stdio transport |
| `http` | `url` | Connect via HTTP |

### `mind` — Mind Runtime (Public Surface)

Only policy toggles exposed to users. Context budgets, memory extraction, character compilation, and extended proactive/emotion knobs use code defaults (see [Cognitive Runtime](../architecture/cognitive-runtime.md)).

```json
{
  "mind": {
    "emotion": { "enabled": true },
    "proactive": {
      "enabled": false,
      "interval_seconds": 60,
      "min_idle_seconds": 120,
      "cooldown_seconds": 300
    }
  }
}
```

#### `mind.emotion`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable emotion processing |
| `classifier_language` | string | `"en"` | Prompt language for the affect classifier (`"en"` or `"ja"`) |

The affect classifier model is routed via `ai.tasks.classifier` (falls back to `ai.tasks.chat` when `null`).

#### `mind.proactive` — Proactive Companion Speech

Policy for unsolicited companion utterances. Default is **off**. Model routing is under `ai.tasks` (see [Proactive Speech ADR](../architecture/proactive-speech.md)).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Master switch |
| `interval_seconds` | int | `60` | Decision tick interval (minimum 1) |
| `min_idle_seconds` | int | `120` | Suppress until this idle after last user input |
| `cooldown_seconds` | int | `300` | Suppress after a successful proactive utterance (`TerminalReason::Done`) |

Extended proactive settings (source flags, confidence gate, timeouts, tool allowance) use code defaults.

### `desktop` — GUI Settings

GUI-specific settings for `ene-desktop` only.

```json
{
  "desktop": {
    "language": "en",
    "graphics": { "quality": "medium" }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `language` | string | `"en"` | UI language: `"en"` or `"ja"` |
| `graphics.quality` | string | `"medium"` | Graphics preset: `"low"`, `"medium"`, or `"high"` |

The quality preset maps to concrete renderer knobs (FPS, shadow map size, antialiasing, mask downsample) at runtime. Individual graphics fields are not user-configurable in the public schema.

## Internal Defaults (Not User-Facing)

The following are controlled by code defaults and are intentionally absent from `settings.json` and the generated schema:

| Area | Examples |
|------|----------|
| `runtime_rules` | Overlay behavioural instructions (`DEFAULT_RUNTIME_RULES`) |
| `session` | Auto-split, summarization model overrides |
| `mind.context` | Token budgets, rolling compression thresholds |
| `mind.memory` | Extraction, hybrid recall, MMR (memory HyDE/LLM rerank intentionally omitted) |
| `mind.character` | CCv3 compilation, identity kernel budget |
| `mind.emotion` / `mind.proactive` | Engine mode, classifier timeouts, source flags, confidence gates |
| `tools` | `max_rounds`, `timeout_ms` |
| `store` | `db_path` |

## Loading Order

1. `EneConfig::default()` — compile-time defaults
2. `assets/settings.json` (or OS user config) — user overrides
3. Environment variables (`ENE_` prefix, `__` separator for nesting)

Example: `ENE_AI__TASKS__CHAT__MODEL=gpt-4o` overrides `ai.tasks.chat.model`.

After loading, `settings.schema.json` and `character_settings.schema.json` are auto-generated into `assets/schema/`.

## JSON Schema

A `settings.schema.json` is auto-generated on `cargo run -p ene-cli` (or any build) and written to `assets/schema/settings.schema.json`. This file is gitignored — do not commit or hand-edit it.

The schema can be used for editor validation (VS Code `"json.schemas"` config) or programmatic config construction.

## Config Registration API

The config system is built on declarative macros and a global schema registry. Each config section is defined with a macro that auto-generates `Serialize`, `Deserialize`, `JsonSchema`, `Default`, and `HasConfigKey` implementations, and registers its schema at program startup via `#[ctor]`.

### `define_config!`

The primary macro for defining config structs. It comes in three forms:

#### Top-level settings section

```rust
ene_config::define_config!(
    settings,          // target: ConfigTarget::Settings
    "ai",              // JSON key under EneConfig.extra
    /// AI provider registry and per-task routing.
    pub struct AiConfig {
        pub providers: BTreeMap<String, AiProviderDef> = default_providers(),
        pub tasks: AiTasksConfig,
    }
);
```

Generates:
- `#[derive(Serialize, Deserialize, JsonSchema)]` with `#[serde(rename_all = "snake_case", default)]`
- `impl Default` using inline `= default_value` syntax (or `Default::default()` if omitted)
- `impl HasConfigKey` with `KEY = "ai"`, `TARGET = Settings`, `path() = ["ai"]`
- `#[ctor]` function that calls `__register_schema::<AiConfig>(Settings, None)`

#### Top-level character section

```rust
ene_config::define_config!(
    character,         // target: ConfigTarget::Character
    "expressions",     // JSON key in character_settings.json
    pub struct ExpressionsConfig {
        pub entries: Vec<ExpressionEntry> = vec![],
    }
);
```

Same as above but `TARGET = Character` and schema is registered for `character_settings.json`.

#### Nested section (child of another config struct)

```rust
ene_config::define_config!(
    AiConfig,          // parent struct (must impl HasConfigKey)
    "api_key",         // JSON key under ai.providers.*.api_key
    pub struct ApiKeyConfig {
        pub source: String = "env".to_string(),
        pub env: String = "OPENAI_API_KEY".to_string(),
    }
);
```

Inherits `TARGET` from the parent. `path()` returns the parent's path + own key (e.g. `["ai", "api_key"]`). The `#[ctor]` call passes the parent key so the schema is nested correctly.

### `define_tool_config!`

For tool-specific config schemas (flattened into `tools.list.<name>`):

```rust
ene_config::define_tool_config!(
    "fs",              // tool name
    /// Sandbox configuration for the fs tool.
    pub struct SandboxConfigData {
        pub enabled: bool = true,
        pub allowed_directories: Vec<String> = vec![".".to_string()],
    }
);
```

Generates the same derives/defaults but calls `__register_tool_schema::<T>("fs")` instead. The schema is registered under `parent_key = "tools_map"` and merged into the `ToolConfig` definition's `list` property in the generated JSON Schema.

### `HasConfigKey` Trait

```rust
pub trait HasConfigKey {
    const KEY: &'static str;       // JSON key (e.g. "ai")
    const TARGET: ConfigTarget;    // Settings or Character
    fn path() -> &'static [&'static str]; // Full path from root (e.g. ["ai", "tasks"])
}
```

Implemented automatically by `define_config!`. Used by:
- `EneConfig::get_section::<T>()` / `set_section()` — type-safe sub-section access
- `ConfigStore::get_section::<T>()` / `set_section()` — same via the store
- `get_global_section::<T>()` — reads directly from the global singleton
- `update_section::<T>()` — load → patch → save in one call

### `ConfigTarget`

```rust
pub enum ConfigTarget {
    Settings,   // belongs to settings.json
    Character,  // belongs to character_settings.json
}
```

Determines which JSON file and schema the config section targets.

### Schema Registry

A global `OnceLock<Mutex<HashMap<String, SchemaEntry>>>` collects all config schemas at startup:

```rust
pub struct SchemaEntry {
    pub schema: schemars::Schema,
    pub target: ConfigTarget,
    pub parent_key: Option<String>,  // None = top-level, Some("tools_map") = tool config
}
```

Registration functions:

| Function | Called by | Purpose |
|----------|-----------|---------|
| `__register_schema::<T>(target, parent_key)` | `#[ctor]` from `define_config!` | Register a settings/character section schema |
| `__register_tool_schema::<T>(tool_name)` | `#[ctor]` from `define_tool_config!` | Register a tool-specific config schema |
| `register_runtime_schema(key, schema_json)` | Runtime (e.g. MCP tool providers) | Register a schema dynamically |

During `generate_schema_json()`, the registry is merged into the root `EneConfig` schema:
- **Top-level sections** (`parent_key = None`) are added as `properties` of the root schema.
- **Tool configs** (`parent_key = "tools_map"`) are injected into `ToolConfig`'s `list` property as `allOf: [ToolEntry, <tool schema>]`.
- **Definitions** (`$defs`) from each entry are copied into the root schema's definitions.

### `ConfigStore`

Centralized persistence layer with dirty tracking for auto-save:

```rust
pub struct ConfigStore {
    config: RwLock<EneConfig>,
    character_config: RwLock<CharacterConfig>,
    global_dirty: AtomicBool,
    character_dirty: AtomicBool,
}
```

Key methods:

| Method | Description |
|--------|-------------|
| `ConfigStore::load()` | Load from disk via figment pipeline |
| `config()` / `set_config()` | Get/replace the global config |
| `with_config_mut(f)` | Mutable access via closure (auto-marks dirty) |
| `get_section::<T>()` / `set_section(&T)` | Type-safe section read/write |
| `character_config()` / `set_character_config()` | Per-character config access |
| `load_character_config(name)` | Load character settings from disk |
| `flush_if_dirty(name)` | Save to disk only if modified (returns `Ok(true)` if wrote) |
| `flush(name)` | Force-save regardless of dirty state |
| `is_dirty()` | Check if any config has unsaved changes |

Typical usage in a game loop (e.g. Bevy):

```rust
fn auto_save(store: Res<ConfigStore>, character: Res<CharacterName>) {
    let _ = store.flush_if_dirty(Some(&character.0));
}
```

### Adding a New Config Section (Checklist)

1. **Define the struct** with `define_config!(settings, "my_key", ...)` in the appropriate crate.
2. **Run `cargo build`** — the `#[ctor]` auto-registers the schema.
3. **Run `cargo run -p ene-cli`** once to regenerate `assets/schema/settings.schema.json`.
4. **Document** the new section in `docs/reference/configuration/settings.md` and `docs/ja/reference/configuration/settings.md`.
5. **Access** via `config.get_section::<MyConfig>()` or `store.get_section::<MyConfig>()`.

## Debug Overlays (per-session, not persisted)

The following overlays are **not** part of the persisted configuration — they reset to their default (off) on every launch. They live on the runtime `UiState` (in `apps/ene-desktop-v2/src/settings.rs`) and are toggled from the Debug settings page or a hotkey on the character window.

| Overlay | Default | Hotkey | Settings control | Effect |
|---------|---------|--------|------------------|--------|
| **Raycast Colliders (Debug)** | `false` (off) | `F3` | "Raycast Colliders (Debug)" checkbox on the Debug page | Draws a wireframe sphere per PR5.2 bone collider (cyan when idle, yellow for the collider under the cursor) and a 3-axis cross at the raycast hit point (red). Built on `ene_vrm::DebugRenderer` (line-list, 3D depth-tested). |
| **Input Region (Debug)** | `false` (off) | `F9` | "Input Region (Debug)" checkbox on the Debug page | Draws the actual input region rectangles pushed to the OS display server (Wayland/X11) as orange wireframes (or red/green/yellow borders for special modes like empty/freeze/full-window). |
| **Mask Overlay (Debug)** | `false` (off) | None | "Mask Overlay (Debug)" checkbox on the Debug page (Linux-only) | Draws the offscreen mask capture wireframe rectangles in purple (Linux-only). |
