# Configuration

ene settings are centralized in `assets/settings.json`. A `settings.schema.json` is auto-generated for editor validation.

Loading: `ene_config::load_full_config()` resolves defaults, file, and environment variables.

## Top-Level Structure (`EneConfig`)

```rust
pub struct EneConfig {
    pub version: u32,           // Currently 1
    pub character: String,      // Character card path or name
    pub user_name: String,      // Default "User"
    pub runtime_rules: String,  // Default system instructions
    pub extra: HashMap<String, serde_json::Value>, // Section map
}
```

### Character Resolution Rules
- Empty string → `assets_dir/characters/Alicia/character.json`
- Name without path separator → `assets_dir/characters/{name}/character.json`

## Sections

### `provider` — AI Provider Connection

```json
{
  "provider": {
    "name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": {
      "source": "inline",
      "inline": "",
      "env": "OPENAI_API_KEY"
    },
    "embedding": {
      "backend": "cloud",
      "query_prefix": null,
      "cloud": {
        "model": "text-embedding-3-small",
        "dimensions": 1536
      },
      "local": {
        "model": "jina-embeddings-v5-text-small",
        "quantization": "F16"
      }
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | `"openai-compatible"` | Provider identifier |
| `model` | string | `"gpt-4o-mini"` | Chat model name |
| `base_url` | string | `""` | API base URL |
| `api_key` | object | (see below) | API key configuration |
| `embedding` | object | (see below) | Embedding configuration |

#### `provider.api_key` — API Key Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `source` | string | `"inline"` | Key source: `"inline"` or `"env"` |
| `inline` | string | `""` | API key when `source = "inline"` (use with caution) |
| `env` | string | `"OPENAI_API_KEY"` | Env var name when `source = "env"` |

#### `provider.embedding` — Embedding Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `backend` | string | `"cloud"` | `"cloud"` uses the provider's embedding API; `"local"` uses a local GGUF model |
| `query_prefix` | string or null | `null` | Optional prefix prepended to search queries |
| `cloud` | object | (see below) | Cloud embedding model config |
| `local` | object | (see below) | Local GGUF embedding config |

##### `provider.embedding.cloud` — Cloud Embedding Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"text-embedding-3-small"` | Cloud embedding model |
| `dimensions` | int | `1536` | Expected dimensions for cloud embedding vectors |

##### `provider.embedding.local` — Local Embedding Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `"jina-embeddings-v5-text-small"` | Local GGUF embedding model name |
| `quantization` | string | `"F16"` | Quantization level (e.g. `"F16"`, `"Q4_K_M"`) |

### `memory` — Long-Term Memory

```json
{
  "memory": {
    "enabled": false,
    "db_path": "",
    "recall_limit": 5,
    "similarity_threshold": 0.5,
    "time_decay_hours": 24.0,
    "similarity_weight": 0.7,
    "recency_weight": 0.3
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable long-term memory |
| `db_path` | string | `""` | SQLite database path (empty = default location) |
| `recall_limit` | int | `5` | Max summaries to recall per query |
| `similarity_threshold` | float | `0.5` | Minimum cosine similarity for recall |
| `time_decay_hours` | float | `24.0` | Hours before recency decays |
| `similarity_weight` | float | `0.7` | Weight for similarity score in recall ranking |
| `recency_weight` | float | `0.3` | Weight for recency score in recall ranking |

### `session` — Session Management

```json
{
  "session": {
    "auto_split": true,
    "timeout_minutes": 30,
    "topic_similarity_threshold": 0.5,
    "min_turns_before_split": 3,
    "recall_limit": 3,
    "summarization": {
      "model": "",
      "base_url": ""
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_split` | bool | `true` | Enable automatic session splitting |
| `timeout_minutes` | int | `30` | Idle timeout before split |
| `topic_similarity_threshold` | float | `0.5` | Cosine similarity threshold for topic drift detection (0.0–1.0) |
| `min_turns_before_split` | int | `3` | Minimum turns before any split can occur |
| `recall_limit` | int | `3` | Max summaries to inject into the prompt |
| `summarization` | object | (see below) | Summarization model configuration |

#### `session.summarization` — Summarization Model Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `model` | string | `""` | Model for summarization (empty = uses chat model) |
| `base_url` | string | `""` | Base URL for summarization (empty = uses chat base URL) |

### `tools` — Tool Configuration

```json
{
  "tools": {
    "enabled": true,
    "max_rounds": 10,
    "timeout_ms": 60000,
    "list": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    },
    "mcp_servers": [],
    "rag": {
      "enabled": true,
      "top_k": 12,
      "final_n": 6,
      "use_hyde": true,
      "use_rerank": true,
      "rerank_candidates": 24,
      "min_similarity": 0.25,
      "background_index_on_startup": false,
      "forced": [
        "utility.question",
        "utility.todo_add",
        "utility.get_current_time"
      ],
      "weights": {
        "summary": 1.0,
        "description": 0.6,
        "capability": 0.8,
        "example": 0.4,
        "negative": -0.5,
        "hyde": 0.7
      }
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable function calling for all tools |
| `max_rounds` | int | `10` | Max tool-call iterations per user turn |
| `timeout_ms` | int | `60000` | Timeout for individual tool calls in milliseconds |
| `list` | object | (see below) | Per-tool enable/disable map with optional extra config |
| `mcp_servers` | array | `[]` | MCP servers list |
| `rag` | object | (see below) | Tool RAG configuration |

#### `tools.list` — Enabled Tools Map

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `<name>.enable` | bool | `true` | Enable/disable a specific tool |
| `<name>.config` | object | `{}` | Optional tool-specific config (flattened into the entry) |

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
| `name` | string | Server name (used for display and routing) |
| `enabled` | bool | Whether this MCP server is enabled |
| `transport` | object | Transport configuration (see below) |

**Transport types:**

| Type | Fields | Description |
|------|--------|-------------|
| `stdio` | `command`, `args` | Spawn a child process with stdio transport |
| `http` | `url` | Connect via HTTP |

#### `tools.rag` — Tool RAG Pipeline

Tool RAG dynamically selects only user-input-relevant tools to reduce token consumption.

```json
{
  "rag": {
    "enabled": true,
    "top_k": 12,
    "final_n": 6,
    "use_hyde": true,
    "use_rerank": true,
    "rerank_candidates": 24,
    "min_similarity": 0.25,
    "background_index_on_startup": false,
    "forced": ["utility.question", "utility.todo_add", "utility.get_current_time"],
    "weights": {
      "summary": 1.0,
      "description": 0.6,
      "capability": 0.8,
      "example": 0.4,
      "negative": -0.5,
      "hyde": 0.7
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable Tool RAG |
| `top_k` | int | `12` | Number of candidates to retrieve from the vector index |
| `final_n` | int | `6` | Final number of tools returned after reranking |
| `use_hyde` | bool | `true` | Use Hypothetical Document Embedding to expand the query |
| `use_rerank` | bool | `true` | Use LLM-based reranking on the candidate set |
| `rerank_candidates` | int | `24` | Number of candidates to pass to the reranker |
| `min_similarity` | float | `0.25` | Minimum similarity score for a tool to be considered |
| `background_index_on_startup` | bool | `false` | Warm the index at startup in a background task |
| `forced` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | Tools always included regardless of relevance |
| `weights` | object | (see below) | Per-field weighting for multi-vector similarity |

##### `tools.rag.weights` — Field Weights

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `summary` | float | `1.0` | Weight for the tool summary embedding |
| `description` | float | `0.6` | Weight for the tool description embedding |
| `capability` | float | `0.8` | Weight for the tool capability embedding |
| `example` | float | `0.4` | Weight for the tool example embedding |
| `negative` | float | `-0.5` | Weight for the negative/unwanted embedding (penalizes matches) |
| `hyde` | float | `0.7` | Weight for the HyDE (hypothetical document embedding) |

### `web_config` — Web Search Providers

API keys for web search providers used by the web tool. This is a tool-specific config injected at runtime.

```json
{
  "web_config": {
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

### `desktop` — GUI Settings

GUI-specific settings for the desktop application. Only available when running `ene-desktop`.

```json
{
  "desktop": {
    "graphics": {
      "mask_render_downsample": 1,
      "target_fps": 60,
      "shadow_quality": "medium",
      "antialiasing_mode": "msaa_4x",
      "debug_fps": 30
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `graphics.mask_render_downsample` | int | `1` | Downsample factor for mask rendering |
| `graphics.target_fps` | int | `60` | Target frames per second |
| `graphics.shadow_quality` | string | `"medium"` | Shadow quality level |
| `graphics.antialiasing_mode` | string | `"fxaa"` | Antialiasing mode |
| `graphics.debug_fps` | int | `30` | Debug update throttle rate (FPS; 0 = no throttle) |

### `cognition` — Cognitive Runtime

Configuration for the Ene Cognitive Runtime, controlling context budget, memory extraction/retention, emotion processing, and character compilation.

> **Note:** This section is part of the [Ene Cognitive Runtime](../architecture/cognitive-runtime.md) redesign. When `cognition.enabled` is `true`, the cognitive runtime replaces the legacy streaming pipeline (planned Phase 10 integration).

```json
{
  "cognition": {
    "enabled": true,
    "context": {
      "max_prompt_tokens": 12000,
      "recent_turns": 8,
      "scene_summary_tokens": 800,
      "memory_budget_tokens": 1800,
      "semantic_budget_tokens": 1200,
      "style_example_budget_tokens": 600
    },
    "memory": {
      "write_every_turn": true,
      "hybrid_search": true,
      "decay_enabled": true,
      "default_forgetting_half_life_days": 30.0,
      "min_confidence_to_persist": 0.65,
      "extraction_timeout_secs": 30,
      "use_hyde": false,
      "recall_result_limit": 8,
      "recall_similarity_threshold": 0.35,
      "recall_min_score": 0.20
    },
    "emotion": {
      "enabled": true,
      "engine": "hybrid",
      "decay_half_life_minutes": 30.0,
      "expression_hysteresis_seconds": 4.0,
      "llm_can_propose_expression": true,
      "llm_expression_is_advisory": true
    },
    "character": {
      "compile_ccv3_to_semantic_memory": true,
      "always_include_identity_kernel": true,
      "style_retrieval": true
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the cognitive runtime. When false, falls back to the legacy streaming pipeline. |

#### `cognition.context` — Context Budget

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_prompt_tokens` | int | `12000` | Maximum total prompt tokens across all sections |
| `recent_turns` | int | `8` | Number of recent conversation turns in the prompt |
| `scene_summary_tokens` | int | `800` | Token budget for the scene/summary section |
| `memory_budget_tokens` | int | `1800` | Token budget for recalled memories |
| `semantic_budget_tokens` | int | `1200` | Token budget for semantic (lorebook) memory |
| `style_example_budget_tokens` | int | `600` | Token budget for style examples from CCv3 lorebook |

#### `cognition.memory` — Memory Extraction & Retention

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `write_every_turn` | bool | `true` | Extract and persist memory on every turn |
| `hybrid_search` | bool | `true` | Use hybrid search (vector + recency + salience + confidence) |
| `decay_enabled` | bool | `true` | Enable time-based memory decay |
| `default_forgetting_half_life_days` | float | `30.0` | Default half-life in days for memory decay |
| `min_confidence_to_persist` | float | `0.65` | Minimum confidence threshold (0.0–1.0) for persisting a memory |
| `extraction_timeout_secs` | int | `30` | Timeout in seconds for a single LLM memory-extraction call; on timeout the extraction fails and falls back to deterministic candidates |
| `use_hyde` | bool | `false` | Record a HyDE query-expansion hint in cognitive recall plans; downstream recall execution performs the provider call |
| `recall_result_limit` | int | `8` | Maximum typed-memory results requested by `RecallPlan` |
| `recall_similarity_threshold` | float | `0.35` | Minimum vector similarity for vector-sourced recall candidates |
| `recall_min_score` | float | `0.20` | Minimum hybrid score required for recalled memory results |

#### `cognition.emotion` — Emotion Engine

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable emotion processing |
| `engine` | string | `"hybrid"` | Engine mode: `"deterministic"`, `"llm"`, or `"hybrid"` |
| `decay_half_life_minutes` | float | `30.0` | Half-life in minutes for affect decay |
| `expression_hysteresis_seconds` | float | `4.0` | Minimum seconds between expression changes (prevents flickering) |
| `llm_can_propose_expression` | bool | `true` | Allow the LLM to propose expression tokens |
| `llm_expression_is_advisory` | bool | `true` | Treat LLM expression proposals as advisory only (not commands) |

#### `cognition.character` — Character Compilation

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `compile_ccv3_to_semantic_memory` | bool | `true` | Compile CCv3 lorebook entries into the semantic memory index |
| `always_include_identity_kernel` | bool | `true` | Always include the Identity Kernel at the top of every prompt |
| `style_retrieval` | bool | `true` | Enable retrieval of character style examples from lorebook |

## Tool-Specific Configuration

Tool-specific settings are stored inside `tools.tools.<name>.config` and vary per tool.

### `tools.tools.fs.config` — Sandbox Configuration

The `fs` tool exposes sandbox controls for file system access:

```json
{
  "tools": {
    "tools": {
      "fs": {
        "enable": true,
        "config": {
          "enabled": true,
          "allowed_directories": ["/home/user/projects"],
          "writable_directories": ["/home/user/projects"],
          "blocked_commands": ["rm -rf /", "dd if=", "mkfs", "sudo"],
          "max_read_bytes": 51200,
          "max_write_bytes": 1048576,
          "shell_timeout_ms": 120000,
          "max_shell_output_bytes": 51200,
          "max_shell_output_lines": 2000
        }
      }
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable sandbox |
| `allowed_directories` | string[] | `["."]` | Directories allowed for read access |
| `writable_directories` | string[] | `["."]` | Directories allowed for write access |
| `blocked_commands` | string[] | (see code) | Regex patterns for blocked shell commands |
| `max_read_bytes` | int | `51200` | Maximum bytes per read operation |
| `max_write_bytes` | int | `1048576` | Maximum bytes per write operation |
| `shell_timeout_ms` | int | `120000` | Shell command timeout in milliseconds |
| `max_shell_output_bytes` | int | `51200` | Maximum bytes in shell output |
| `max_shell_output_lines` | int | `2000` | Maximum lines in shell output |

## Loading Order

1. `EneConfig::default()` — compile-time defaults
2. `assets/settings.json` — user overrides
3. Environment variables (`ENE_` prefix, `__` separator for nesting)

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
    "provider",        // JSON key under EneConfig.extra
    /// AI provider connection config.
    pub struct ProviderConfig {
        /// Provider name.
        pub name: String = "openai-compatible".to_string(),
        /// Chat model name.
        pub model: String = "gpt-4o-mini".to_string(),
    }
);
```

Generates:
- `#[derive(Serialize, Deserialize, JsonSchema)]` with `#[serde(rename_all = "snake_case", default)]`
- `impl Default` using inline `= default_value` syntax (or `Default::default()` if omitted)
- `impl HasConfigKey` with `KEY = "provider"`, `TARGET = Settings`, `path() = ["provider"]`
- `#[ctor]` function that calls `__register_schema::<ProviderConfig>(Settings, None)`

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
    EmbeddingConfig,   // parent struct (must impl HasConfigKey)
    "local",           // JSON key under provider.embedding.*
    pub struct LocalEmbeddingConfig {
        pub model: String = "jina-embeddings-v5-text-small".to_string(),
        pub quantization: String = "F16".to_string(),
    }
);
```

Inherits `TARGET` from the parent. `path()` returns the parent's path + own key (e.g. `["provider", "local_embedding"]`). The `#[ctor]` call passes the parent key so the schema is nested correctly.

### `define_tool_config!`

For tool-specific config schemas (injected into `tools.tools.<name>.config`):

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

Generates the same derives/defaults but calls `__register_tool_schema::<T>("fs")` instead. The schema is registered under `parent_key = "tools_map"` and merged into the `ToolConfig` definition's `tools` property in the generated JSON Schema.

### `HasConfigKey` Trait

```rust
pub trait HasConfigKey {
    const KEY: &'static str;       // JSON key (e.g. "provider")
    const TARGET: ConfigTarget;    // Settings or Character
    fn path() -> &'static [&'static str]; // Full path from root (e.g. ["provider", "local_embedding"])
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
- **Tool configs** (`parent_key = "tools_map"`) are injected into `ToolConfig`'s `tools` property as `allOf: [ToolEntry, <tool schema>]`.
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
4. **Document** the new section in `docs/configuration/settings.md` and `docs/ja/configuration/settings.md`.
5. **Access** via `config.get_section::<MyConfig>()` or `store.get_section::<MyConfig>()`.

## Debug Overlays (per-session, not persisted)

The following overlays are **not** part of the persisted configuration — they reset to their default (off) on every launch. They live on the runtime `UiState` (in `apps/ene-desktop-v2/src/settings.rs`) and are toggled from the Debug settings page or a hotkey on the character window.

| Overlay | Default | Hotkey | Settings control | Effect |
|---------|---------|--------|------------------|--------|
| **Raycast Colliders (Debug)** | `false` (off) | `F3` | "Raycast Colliders (Debug)" checkbox on the Debug page | Draws a wireframe sphere per PR5.2 bone collider (cyan when idle, yellow for the collider under the cursor) and a 3-axis cross at the raycast hit point (red). Built on `ene_vrm::DebugRenderer` (line-list, 3D depth-tested). |
| **Input Region (Debug)** | `false` (off) | `F9` | "Input Region (Debug)" checkbox on the Debug page | Draws the actual input region rectangles pushed to the OS display server (Wayland/X11) as orange wireframes (or red/green/yellow borders for special modes like empty/freeze/full-window). |
| **Mask Overlay (Debug)** | `false` (off) | None | "Mask Overlay (Debug)" checkbox on the Debug page (Linux-only) | Draws the offscreen mask capture wireframe rectangles in purple (Linux-only). |
