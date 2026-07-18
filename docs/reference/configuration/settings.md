# Configuration

ene settings are centralized in `assets/settings.json`. A `settings.schema.json` is auto-generated for editor validation.

Loading: `ene_config::load_full_config()` / `ConfigStore` resolves defaults, file, and environment variables.

**API v2 ownership:** persistence toggles live under `store` (`enabled`, `db_path` only). Recall / write / decay / MMR / emotion / performance policy knobs live under `mind.*` (including `mind.memory.*`). There is no top-level `memory.*` policy section and no `cognition.enabled` dual-pipeline switch — the mind path is the only streaming path.

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
    "max_tokens": 8192,
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
| `max_tokens` | int | `8192` | Max completion tokens for chat (`0` = omit from the request). OpenRouter reserves credit collateral against this ceiling; omitting it can make the provider assume the model max (often 65536) and return HTTP 402 on modest balances |
| `api_key` | object | (see below) | API key configuration |
| `embedding` | object | (see below) | Embedding configuration |
| `proactive` | object | (see below) | Proactive companion speech model routing (#103) |

#### `provider.proactive` — Proactive Model Routing

```json
{
  "proactive": {
    "decision": {
      "backend": "disabled",
      "model_path": "",
      "executable": "",
      "acceleration": "auto",
      "gpu_layers": "auto",
      "context_size": 2048,
      "startup_timeout_seconds": 60,
      "request_timeout_seconds": 20,
      "fallback": "disabled",
      "cloud_model": ""
    },
    "generation_model": ""
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `decision.backend` | string | `"disabled"` | `"llama_cpp"`, `"cloud"`, or `"disabled"` |
| `decision.model_path` | string | `""` | Path to decision GGUF weights (required for `llama_cpp`) |
| `decision.executable` | string | `""` | Path to `llama-server` (empty = `PATH`) |
| `decision.acceleration` | string | `"auto"` | `"auto"`, `"vulkan"`, `"cuda"`, or `"cpu"` |
| `decision.gpu_layers` | string | `"auto"` | `"auto"` or an integer string for `--n-gpu-layers` |
| `decision.context_size` | int | `2048` | Small context for decision prompts |
| `decision.startup_timeout_seconds` | int | `60` | Wait for local server health |
| `decision.request_timeout_seconds` | int | `20` | Per-decision request timeout |
| `decision.fallback` | string | `"disabled"` | On local failure: `"disabled"` or `"cloud"` (never silent cloud upload when disabled) |
| `decision.cloud_model` | string | `""` | Optional cloud model override for decision |
| `generation_model` | string | `""` | Proactive utterance model; empty uses `provider.model` |

Connection credentials reuse `provider.base_url` / `provider.api_key`. Binary and GGUF weights are not bundled; see the [Proactive Speech ADR](../architecture/proactive-speech.md).

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

### `store` — Persistent SQLite-vec Store

```json
{
  "store": {
    "enabled": false,
    "db_path": ""
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable the persistence store |
| `db_path` | string | `""` | SQLite database path (empty = default location) |

### `session` — Session Management

```json
{
  "session": {
    "auto_split": false,
    "timeout_minutes": 30,
    "topic_similarity_threshold": 0.5,
    "min_turns_before_split": 3,
    "summarization": {
      "model": "",
      "base_url": ""
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_split` | bool | `false` | **Deprecated hard-split path.** Default is off. Prefer rolling compression via `mind.context.compression_*` (see below). When `true`, composite scoring may mint a new session ID — not the product path. |
| `timeout_minutes` | int | `30` | Idle timeout before split |
| `topic_similarity_threshold` | float | `0.5` | Cosine similarity threshold for topic drift detection (0.0–1.0) |
| `min_turns_before_split` | int | `3` | Minimum turns before any split can occur |
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
      "background_index_on_startup": true,
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
    "background_index_on_startup": true,
    "forced": ["utility.question", "utility.todo_add", "utility.get_current_time"],
    "weights": {
      "summary": 1.0,
      "description": 0.6,
      "capability": 0.8,
      "example": 0.4,
      "negative": -0.5,
      "hyde": 0.7,
      "hyde_blend": 0.6
    },
    "per_category_limits": {}
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
| `background_index_on_startup` | bool | `true` | Warm the tool embedding index during runtime bootstrap (Phase 3) |
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

### `mind` — Mind Runtime

Configuration for the Ene Cognitive Runtime, controlling context budget, memory extraction/retention, emotion processing, and character compilation.

> **Note:** This section is part of the [Ene Cognitive Runtime](../architecture/cognitive-runtime.md). The mind runtime is the sole streaming path; missing store/embedder prerequisites fail closed.

```json
{
  "mind": {
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
      "llm_extraction_enabled": true,
      "semantic_dedup_enabled": true,
      "hybrid_search": true,
      "decay_enabled": true,
      "default_forgetting_half_life_days": 30.0,
      "min_confidence_to_persist": 0.65,
      "extraction_timeout_secs": 30,
      "tool_grounding": {
        "enabled": true,
        "max_summary_chars": 500,
        "persist_success_procedure": false,
        "persist_failure_reflection": true,
        "persist_user_visible_episodic": false,
        "min_confidence": 0.60
      },
      "use_hyde": false,
      "hyde_blend": 0.6,
      "recall_result_limit": 8,
      "recall_similarity_threshold": 0.35,
      "recall_min_score": 0.20,
      "rerank_enabled": false,
      "rerank_candidate_limit": 16,
      "rerank_timeout_secs": 10,
      "mmr_enabled": true,
      "mmr_lambda": 0.7,
      "mmr_duplicate_cluster_threshold": 0.75,
      "mmr_min_slots_semantic": 1,
      "mmr_min_slots_episodic": 1,
      "mmr_min_slots_user_profile": 1,
      "mmr_min_slots_commitment": 1,
      "mmr_source_diversity_bonus": 0.05,
      "require_migration": false
    },
    "emotion": {
      "enabled": true,
      "engine": "hybrid",
      "decay_half_life_minutes": 30.0,
      "expression_hysteresis_seconds": 4.0,
      "llm_can_propose_expression": true,
      "llm_expression_is_advisory": true,
      "classifier_timeout_secs": 15,
      "classifier_min_confidence": 0.5,
      "classifier_language": "en"
    },
    "character": {
      "compile_ccv3_to_semantic_memory": true,
      "always_include_identity_kernel": true,
      "identity_kernel_max_tokens": 400,
      "style_retrieval": true
    },
    "proactive": {
      "enabled": false,
      "interval_seconds": 60,
      "min_idle_seconds": 120,
      "cooldown_seconds": 300,
      "max_turns_per_session": 6,
      "decision_timeout_seconds": 15,
      "generation_timeout_seconds": 60,
      "sources": {
        "conversation": true,
        "activity": true,
        "screen_summary": false
      },
      "decision": {
        "min_confidence": 0.55
      },
      "allow_tools": false,
      "max_conversation_chars": 4000,
      "max_activity_chars": 500,
      "max_screen_summary_chars": 800
    }
  }
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|

#### `mind.proactive` — Proactive Companion Speech

Policy for unsolicited companion utterances. Default is **off**. See [Proactive Speech ADR](../architecture/proactive-speech.md).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Master switch |
| `interval_seconds` | int | `60` | Decision tick interval (minimum 1) |
| `min_idle_seconds` | int | `120` | Suppress until this idle after last user input |
| `cooldown_seconds` | int | `300` | Suppress after a proactive utterance |
| `max_turns_per_session` | int | `6` | Cap per conversation session |
| `decision_timeout_seconds` | int | `15` | Lightweight decision timeout |
| `generation_timeout_seconds` | int | `60` | High-quality generation timeout |
| `sources.conversation` | bool | `true` | Include recent chat history |
| `sources.activity` | bool | `true` | Include privacy-safe activity / idle |
| `sources.screen_summary` | bool | `false` | Include short-lived screen text summary |
| `decision.min_confidence` | float | `0.55` | Minimum confidence to start generation (`0.0..=1.0`) |
| `allow_tools` | bool | `false` | Allow tool selection during proactive generation |
| `max_conversation_chars` | int | `4000` | Conversation budget in the decision prompt |
| `max_activity_chars` | int | `500` | Activity text budget |
| `max_screen_summary_chars` | int | `800` | Screen summary budget |

#### `mind.context` — Context Budget

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_prompt_tokens` | int | `12000` | Maximum total prompt tokens across all sections |
| `recent_turns` | int | `8` | Number of recent conversation turns in the prompt |
| `scene_summary_tokens` | int | `800` | Token budget for the scene/summary section |
| `memory_budget_tokens` | int | `1800` | Token budget for recalled memories |
| `semantic_budget_tokens` | int | `1200` | Token budget for semantic (lorebook) memory |
| `style_example_budget_tokens` | int | `600` | Token budget for style examples from CCv3 lorebook |
| `compression_enabled` | bool | `true` | **Preferred context boundary.** Enable rolling context compression instead of hard session splits |
| `scene_turn_threshold` | int | `12` | Turn count before scene-level compression is triggered |
| `chapter_span_threshold` | int | `5` | Number of scene spans before chapter rollup |
| `arc_span_threshold` | int | `3` | Number of chapter spans before arc rollup |
| `compression_timeout_secs` | int | `60` | Timeout for a single compression summarization LLM call |

#### `mind.memory` — Memory Extraction & Retention

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `write_every_turn` | bool | `true` | Extract and persist memory on every turn |
| `llm_extraction_enabled` | bool | `true` | Enable LLM-first memory candidate extraction. Deterministic matchers are remember/forget only (remember falls back when LLM fails/empty/disabled; forget always applies). Soft signals are LLM-only |
| `semantic_dedup_enabled` | bool | `true` | Pre-arbitration semantic duplicate detection via embedding search (#75) |
| `hybrid_search` | bool | `true` | Use hybrid search (vector + recency + salience + confidence) |
| `decay_enabled` | bool | `true` | Enable post-turn natural decay (`Active → Faded → Archived`) via `ForgettingLifecycle` |
| `default_forgetting_half_life_days` | float | `30.0` | Half-life in days for lifecycle decay score and recall recency scoring |
| `min_confidence_to_persist` | float | `0.65` | Minimum confidence threshold (0.0–1.0) for persisting a memory |
| `extraction_timeout_secs` | int | `30` | Timeout in seconds for a single LLM memory-extraction call; on timeout the extraction fails and falls back to deterministic candidates |
| `tool_grounding` | object | (see below) | Tool-result grounding guardrails and candidate extraction controls (#92) |
| `use_hyde` | bool | `false` | When true, generate a hypothetical document via the LLM, embed it as HyDE, and blend with the query embedding before hybrid recall search |
| `hyde_blend` | float | `0.6` | Fraction of the search vector taken from the HyDE embedding (`0.0`–`1.0`). Ignored when `use_hyde` is false |
| `recall_result_limit` | int | `8` | Maximum typed-memory results requested by `RecallPlan` |
| `recall_similarity_threshold` | float | `0.35` | Minimum vector similarity for vector-sourced recall candidates |
| `recall_min_score` | float | `0.20` | Minimum hybrid score required for recalled memory results |
| `rerank_enabled` | bool | `false` | Enable optional LLM reranking of hybrid recall candidates after search |
| `rerank_candidate_limit` | int | `16` | Maximum number of top hybrid-search candidates sent to the reranker |
| `rerank_timeout_secs` | int | `10` | Timeout in seconds for a single LLM memory-rerank call; on timeout or provider failure the pipeline falls back to hybrid search order |
| `mmr_enabled` | bool | `true` | Enable MMR diversification after hybrid search (#78). When enabled (default), recall candidate order may differ from pure hybrid-score ranking |
| `mmr_lambda` | float | `0.7` | MMR relevance-vs-diversity tradeoff in `0.0`–`1.0`; higher favors relevance |
| `mmr_duplicate_cluster_threshold` | float | `0.75` | Lexical similarity threshold for merging near-duplicate recall candidates |
| `mmr_min_slots_semantic` | int | `1` | Minimum recalled slots reserved for semantic memories |
| `mmr_min_slots_episodic` | int | `1` | Minimum recalled slots reserved for episodic memories |
| `mmr_min_slots_user_profile` | int | `1` | Minimum recalled slots reserved for user profile memories |
| `mmr_min_slots_commitment` | int | `1` | Minimum recalled slots reserved for commitment memories |
| `mmr_source_diversity_bonus` | float | `0.05` | Bonus added to MMR score when a candidate introduces a new recall source type |
| `require_migration` | bool | `false` | When true, block typed recall while legacy summaries/keyfacts exist and migration is incomplete; ongoing `conversation_logs` do not block (#98) |
| `hybrid_weights` | object | (see defaults) | Hybrid scoring weights (`vector`, `lexical`, `recency`, `salience`, `confidence`, `emotional_match`, `relationship`, `access_boost`). Product defaults live here; store only applies caller-provided weights (#123) |
| `commitment_boost` | float | `0.25` | Score boost when a candidate is sourced from an active commitment |
| `recent_fallback_limit` | int | `5` | Max pure-recent fallback candidates gathered during hybrid search |
| `journal_candidate_pool_size` | int | `64` | Candidate pool size for diagnostics/CLI/desktop journal search |
| `journal_similarity_threshold` | float | `0.45` | Minimum vector similarity for journal search |
| `journal_min_score` | float | `0.10` | Minimum hybrid score for journal search |

#### `mind.memory.tool_grounding` — Tool Result Grounding

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable grounding tool call results into cognitive memory |
| `max_summary_chars` | int | `500` | Maximum characters retained per tool summary before truncation |
| `persist_success_procedure` | bool | `false` | Persist successful tool calls as `Procedure` memories (deterministic fallback when LLM extraction does not own the turn; lasting value is normally judged by the LLM extractor) |
| `persist_failure_reflection` | bool | `true` | Persist failed tool calls as `Reflection` memories (fallback when LLM extraction does not own the turn) |
| `persist_user_visible_episodic` | bool | `false` | Persist concise user-visible tool outcomes as `Episodic` memories (deterministic fallback; usually judged by the LLM extractor) |
| `min_confidence` | float | `0.60` | Confidence threshold for tool-derived memory candidates (`0.0`–`1.0`) |

#### `mind.emotion` — Emotion Engine

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `true` | Enable emotion processing |
| `engine` | string | `"hybrid"` | Emotion engine mode. `"deterministic"`: rules + decay only (no post-turn classifier). `"hybrid"`: rules + decay **and** async post-turn classifier (recommended). `"llm"`: decay + classifier only (skips rule-based appraisal) |
| `decay_half_life_minutes` | float | `30.0` | Half-life in minutes for affect decay |
| `expression_hysteresis_seconds` | float | `4.0` | Minimum seconds between expression changes (prevents flickering) |
| `llm_can_propose_expression` | bool | `true` | Allow the LLM to propose expression tokens |
| `llm_expression_is_advisory` | bool | `true` | Treat LLM expression proposals as advisory only (not commands) |
| `classifier_timeout_secs` | int | `30` | Timeout in seconds for the post-turn async LLM affect classifier job (#88); does not block response generation. Uses strict JSON Schema, streaming fallback, and `classifier_max_tokens` |
| `classifier_min_confidence` | float | `0.5` | Minimum classifier confidence to blend absolute LLM affect estimates |
| `classifier_language` | string | `"en"` | Prompt library language for affect classifier and natural-dialogue output contract (`en` or `ja`) |
| `classifier_model` | string | `"google/gemini-2.5-flash-lite"` | Chat model for the affect classifier (OpenRouter slug) |
| `classifier_max_tokens` | int | `0` | Max completion tokens for classifier LLM calls (`0` = no cap) |

#### `mind.character` — Character Compilation

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `compile_ccv3_to_semantic_memory` | bool | `true` | Compile CCv3 lorebook entries into the semantic memory index |
| `always_include_identity_kernel` | bool | `true` | Always include the Identity Kernel at the top of every prompt |
| `identity_kernel_max_tokens` | int | `400` | Approximate token budget for the compiled Identity Kernel (#82). Optional detail sections are dropped first; core header lines are preserved |
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
4. **Document** the new section in `docs/reference/configuration/settings.md` and `docs/ja/reference/configuration/settings.md`.
5. **Access** via `config.get_section::<MyConfig>()` or `store.get_section::<MyConfig>()`.

## Debug Overlays (per-session, not persisted)

The following overlays are **not** part of the persisted configuration — they reset to their default (off) on every launch. They live on the runtime `UiState` (in `apps/ene-desktop-v2/src/settings.rs`) and are toggled from the Debug settings page or a hotkey on the character window.

| Overlay | Default | Hotkey | Settings control | Effect |
|---------|---------|--------|------------------|--------|
| **Raycast Colliders (Debug)** | `false` (off) | `F3` | "Raycast Colliders (Debug)" checkbox on the Debug page | Draws a wireframe sphere per PR5.2 bone collider (cyan when idle, yellow for the collider under the cursor) and a 3-axis cross at the raycast hit point (red). Built on `ene_vrm::DebugRenderer` (line-list, 3D depth-tested). |
| **Input Region (Debug)** | `false` (off) | `F9` | "Input Region (Debug)" checkbox on the Debug page | Draws the actual input region rectangles pushed to the OS display server (Wayland/X11) as orange wireframes (or red/green/yellow borders for special modes like empty/freeze/full-window). |
| **Mask Overlay (Debug)** | `false` (off) | None | "Mask Overlay (Debug)" checkbox on the Debug page (Linux-only) | Draws the offscreen mask capture wireframe rectangles in purple (Linux-only). |
