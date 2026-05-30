# Configuration

ene settings are centralized in `assets/settings.json`. A `settings.schema.json` is auto-generated for editor validation.

Loading: `ene_config::load_full_settings()` resolves defaults, file, and environment variables.

## Top-Level Structure (`EneSettings`)

```rust
pub struct EneSettings {
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

### `provider` — LLM Connection

```json
{
  "provider": {
    "provider_name": "openai-compatible",
    "model": "gpt-4o-mini",
    "base_url": "https://api.openai.com/v1",
    "api_key": ""
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `provider_name` | string | Provider identifier (default: `"openai-compatible"`) |
| `model` | string | Model name (default: `"gpt-4o-mini"`) |
| `base_url` | string | API endpoint (must not be empty for production) |
| `api_key` | string | API key (falls back to `API_TOKEN` env var in debug) |

### `embedding` — Vector Embedding

```json
{
  "embedding": {
    "provider_type": "local",
    "model": "jina-embeddings-v5-text-small",
    "base_url": "",
    "dimensions": null,
    "gguf_quantization": "F16"
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider_type` | `"api"` or `"local"` | `"local"` | Backend type |
| `model` | string | `"jina-embeddings-v5-text-small"` | Model name |
| `base_url` | string | `""` | API URL (API mode only) |
| `dimensions` | int or null | `null` | Output dimensions |
| `gguf_quantization` | string | `"F16"` | GGUF quantization level |

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
    "recency_weight": 0.3,
    "tool_rag_enabled": true,
    "tool_rag_limit": 6,
    "tool_rag_always_include": ["question", "todo", "get_current_time"],
    "summarization_model": "",
    "summarization_base_url": ""
  }
}
```

| Key field | Description |
|-----------|-------------|
| `enabled` | Enable long-term memory |
| `db_path` | SQLite database path (empty = default location) |
| `recall_limit` | Max summaries to recall per query |
| `similarity_threshold` | Minimum cosine similarity for recall |
| `tool_rag_enabled` | Enable embedding-based tool filtering |
| `tool_rag_limit` | Max tools returned by RAG filtering |
| `tool_rag_always_include` | Tools always included regardless of similarity |

### `session` — Session Management

```json
{
  "session": {
    "auto_session_split": true,
    "session_timeout_minutes": 30,
    "topic_change_threshold": 0.5,
    "min_turns_before_split": 3,
    "summary_recall_limit": 3
  }
}
```

| Field | Description |
|-------|-------------|
| `auto_session_split` | Enable automatic session splitting |
| `session_timeout_minutes` | Idle timeout before split |
| `topic_change_threshold` | Cosine similarity threshold for topic drift |
| `min_turns_before_split` | Minimum turns before any split can occur |

### `sandbox` — Tool Security

```json
{
  "sandbox": {
    "enabled": true,
    "allowed_directories": ["/home/user/projects"],
    "writable_directories": ["/home/user/projects"],
    "blocked_commands": ["rm -rf /", "dd if=", "mkfs", "sudo"],
    "max_read_bytes": 51200,
    "max_write_bytes": 1048576,
    "shell_timeout_ms": 120000,
    "max_shell_output_bytes": 51200,
    "max_shell_output_lines": 2000,
    "undo_db_path": null
  }
}
```

### `tools` — Tool Configuration

```json
{
  "tools": {
    "tool_calling_enabled": true,
    "max_tool_call_rounds": 10,
    "tool_call_timeout_ms": 60000,
    "tools": {
      "fs": { "enable": true },
      "web": { "enable": true },
      "browser": { "enable": true },
      "utility": { "enable": true },
      "app": { "enable": true }
    }
  }
}
```

| Field | Description |
|-------|-------------|
| `tool_calling_enabled` | Enable function calling for all tools |
| `max_tool_call_rounds` | Max tool-call iterations per user turn |
| `tool_call_timeout_ms` | Timeout for individual tool calls in milliseconds (default: 60000) |
| `tools.<name>.enable` | Enable/disable a specific tool |
| `tools.<name>.config` | Optional tool-specific config (merged) |

### `mcp_servers` — Model Context Protocol

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

### `desktop` — GUI Settings

```json
{
  "desktop": {
    "graphics": {
      "mask_render_downsample": 1,
      "target_fps": 60,
      "shadow_quality": "medium",
      "antialiasing_mode": "msaa_4x"
    }
  }
}
```

## Loading Order

1. `EneSettings::default()`
2. `assets/settings.json`
3. Environment variables (`ENE_` prefix, `__` separator for nesting)

After loading, `settings.schema.json` and `character_settings.schema.json` are auto-generated.
