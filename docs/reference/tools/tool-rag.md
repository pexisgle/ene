# Tool RAG

Tool RAG (Retrieval-Augmented Generation) selects the most relevant tools for a given user query using vector embeddings. Instead of sending all tools to the LLM, only the top-N most relevant tools are included in the prompt.

Indexing text comes from [`ToolRagProfile`](./sdk.md#toolragprofile) (#137). The LLM still receives slim [`ToolSpec`](./sdk.md#toolspec) values (`name`, `description`, `parameters` only).

## How It Works

```
User query
  ↓
1. Embed query → query_embedding
  ↓
2. For each tool, compute weighted similarity:
   score = Σ (weight_i × cosine_sim(query, tool_field_i))
   where fields are: summary, description, capability, example, negative
  ↓
3. Apply per-category limits (e.g. max 3 Filesystem tools)
  ↓
4. Sort by score, take top_k candidates
  ↓
5. Optional cosine embedding rerank when `use_rerank` and multiple candidates remain → pick final_n
  ↓
6. Always include forced tools regardless of score
  ↓
Vec<ToolSpec> → passed to LLM
```

LLM HyDE is **deprecated** and disabled: `use_hyde` is a no-op scheduled for removal. Invalid `rag.forced` names fail startup. Embed/store failures return forced tools only (not the full catalog). Rerank never uses an LLM (cosine over description embeddings only).

## Configuration

Configure under `tools.rag` in `settings.json` (see [Settings](../configuration/settings.md#toolsrag--tool-rag-pipeline)). Built when `tools.rag.enabled` is true and an embedding provider is available.

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enabled` | bool | `true` | Enable the Tool RAG pipeline |
| `use_hyde` | bool | `false` | **Deprecated** (no-op; scheduled for removal) |
| `use_rerank` | bool | `false` | Cosine embedding rerank of candidates (no LLM) |
| `background_index_on_startup` | bool | `true` | Warm the index in a background task at startup |
| `top_k` | int | `12` | Number of candidates before reranking |
| `final_n` | int | `6` | Final number of tools sent to LLM |
| `rerank_candidates` | int | `24` | Number of candidates considered during embedding rerank |
| `min_similarity` | float | `0.25` | Minimum similarity threshold |
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

## Embedding Rerank

When more than one candidate survives scoring, the pipeline reranks by cosine similarity between the query embedding and each tool's primary field embedding before selecting `final_n` tools. This is deterministic and does not call an LLM.

## Field Weights

| Weight | Description |
|--------|-------------|
| `summary` | How much the tool's summary contributes |
| `description` | How much the full description contributes |
| `capability` | How much the capability embedding contributes |
| `example` | How much examples contribute |
| `negative` | Soft penalty for negative keyword matches (default `-0.5`) |

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
