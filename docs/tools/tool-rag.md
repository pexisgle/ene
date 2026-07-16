# Tool RAG

Tool RAG (Retrieval-Augmented Generation) selects the most relevant tools for a given user query using vector embeddings. Instead of sending all tools to the LLM, only the top-N most relevant tools are included in the prompt.

Indexing text comes from [`ToolRagProfile`](./sdk.md#toolragprofile) (#137). The LLM still receives slim [`ToolSpec`](./sdk.md#toolspec) values (`name`, `description`, `parameters` only).

## How It Works

```
User query
  ↓
1. Embed query → query_embedding
  ↓ (optional) HyDE: generate hypothetical answer → embed → hyde_embedding
  ↓
2. For each tool, compute weighted similarity:
   score = Σ (weight_i × cosine_sim(query_embedding, tool_field_i))
   where fields are: summary, description, capability, example, negative, hyde
  ↓
3. Apply per-category limits (e.g. max 3 Filesystem tools)
  ↓
4. Sort by score, take top_k candidates
  ↓
5. (optional) LLM rerank the top_k → pick final_n
  ↓
6. Always include forced tools regardless of score
  ↓
Vec<ToolSpec> → passed to LLM
```

## Configuration

In `settings.json` under `tools.rag` (config key `rag`, path `["tools", "rag"]`):

```json
{
  "tools": {
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
        "hyde": 0.7,
        "hyde_blend": 0.6
      },
      "per_category_limits": {
        "Filesystem": 3
      }
    }
  }
}
```

### Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | bool | `true` | Enable/disable Tool RAG |
| `top_k` | int | `12` | Number of candidates before reranking |
| `final_n` | int | `6` | Final number of tools sent to LLM |
| `use_hyde` | bool | `true` | Use HyDE (Hypothetical Document Embeddings) |
| `use_rerank` | bool | `true` | Use LLM reranking on top candidates |
| `rerank_candidates` | int | `24` | Number of candidates for LLM reranking |
| `min_similarity` | float | `0.25` | Minimum similarity threshold |
| `background_index_on_startup` | bool | `true` | Warm the tool embedding index during runtime bootstrap |
| `forced` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | Always include these tools |
| `per_category_limits` | map | `{}` | Max tools per `ToolCategory::config_key` (e.g. `"Filesystem"`) |

## Multi-Vector Embedding

Each tool is embedded across multiple fields from its `ToolRagProfile`, stored in `tool_embedding_index`:

| Field | Content | Default Weight |
|-------|---------|---------------|
| `summary` | `"{name}: {summary}"` | 1.0 |
| `description` | description + keywords + JSON Schema property summary | 0.6 |
| `capability` | category label + summary + primary keywords | 0.8 |
| `example` | one row per example (`field_key = ex_N`) | 0.4 |
| `negative` | `"{name} NOT: {negative keywords}"` | -0.5 (penalty) |

The version hash is derived from the text content, so tools are only re-embedded when their content changes. `ensure_index(specs, profiles)` hashes both inputs.

## HyDE (Hypothetical Document Embeddings)

When `use_hyde = true`, the pipeline:
1. Generates a hypothetical answer to the user query using the LLM
2. Embeds the hypothetical answer
3. Blends query and HyDE similarity via `weights.hyde` and `weights.hyde_blend`

This improves recall for queries that describe what they want to achieve rather than naming the tool directly.

## Field Weights

| Weight | Description |
|--------|-------------|
| `summary` | How much the tool's summary contributes |
| `description` | How much the full description contributes |
| `capability` | How much the capability embedding contributes |
| `example` | How much examples contribute |
| `negative` | Soft penalty for negative keyword matches (default `-0.5`) |
| `hyde` | How much the HyDE embedding contributes |
| `hyde_blend` | Fraction of score from HyDE vs direct similarity (`0.0`–`1.0`) |

## Per-Category Limits

Limit how many tools from each category can appear after scoring (lowest scores dropped first):

```json
{
  "per_category_limits": {
    "Filesystem": 3,
    "Browser": 2
  }
}
```

Keys must match `ToolCategory::config_key()` (`Filesystem`, `Shell`, `Browser`, `App`, `WebSearch`, `WebFetch`, `Utility`, `Memory`, `Search`, `Meta`).

## Forced Tools

Tools listed in `forced` are always included regardless of similarity scores. Default forced tools are general-purpose utilities that the LLM should always have access to.

## Architecture

```
Tool binaries
  → ToolProvider::list_specs / list_rag_profiles
  → IpcToolRegistry (ListTools + ListRagProfiles, IPC v4)
  → CompositeToolRegistry
  → ToolRag::ensure_index(specs, profiles)
  → tool_embedding_index (SQLite)
  → ToolRag::select → Vec<ToolSpec> for the LLM
```

MCP tools have no authoring profile; the host synthesizes a minimal `ToolRagProfile` from each `ToolSpec`.
