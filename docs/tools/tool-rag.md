# Tool RAG

Tool RAG (Retrieval-Augmented Generation) selects the most relevant tools for a given user query using vector embeddings. Instead of sending all tools to the LLM, only the top-N most relevant tools are included in the prompt.

## How It Works

```
User query
  ↓
1. Embed query → query_embedding
  ↓ (optional) HyDE: generate hypothetical answer → embed → hyde_embedding
  ↓
2. For each tool, compute weighted similarity:
   score = Σ (weight_i × cosine_sim(query_embedding, tool_field_i))
   where fields are: summary, description, negative, hyde
  ↓
3. Apply per-category limits (e.g. max 3 filesystem tools)
  ↓
4. Sort by score, take top_k candidates
  ↓
5. (optional) LLM rerank the top_k → pick final_n
  ↓
6. Always include forced_tools regardless of score
  ↓
Vec<ToolSpec> → passed to LLM
```

## Configuration

In `settings.json` under `tools`:

```json
{
  "tools": {
    "tool_rag": {
      "enabled": true,
      "top_k": 12,
      "final_n": 6,
      "use_hyde": true,
      "use_rerank": true,
      "rerank_candidates": 24,
      "min_similarity": 0.25,
      "background_index_on_startup": false,
      "forced_tools": [
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
      },
      "per_category_limits": {}
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
| `background_index_on_startup` | bool | `false` | Index tools on startup |
| `forced_tools` | string[] | `["utility.question", "utility.todo_add", "utility.get_current_time"]` | Always include these tools |

## Multi-Vector Embedding

Each tool is embedded across multiple fields, stored separately in `tool_embedding_index`:

| Field | Content | Default Weight |
|-------|---------|---------------|
| `summary` | `"tool.name: one-line summary"` | 1.0 |
| `description` | Full description + keywords | 0.6 |
| `negative` | `"tool.name NOT: negative, keywords"` | -0.5 (penalty) |

The version hash is derived from the text content, so tools are only re-embedded when their content changes.

## HyDE (Hypothetical Document Embeddings)

When `use_hyde = true`, the pipeline:
1. Generates a hypothetical answer to the user query using the LLM
2. Embeds the hypothetical answer
3. Uses both the query embedding and HyDE embedding for scoring

This improves recall for queries that describe what they want to achieve rather than naming the tool directly.

## Field Weights

| Weight | Description |
|--------|-------------|
| `summary` | How much the tool's summary contributes |
| `description` | How much the full description contributes |
| `capability` | Reserved for capability-based matching |
| `example` | How much examples contribute |
| `negative` | Penalty for negative keyword matches (negative = soft penalty) |
| `hyde` | How much the HyDE embedding contributes |

Set `negative < 0` for soft penalty (tool still appears, ranked lower). Set `negative > 0` for hard exclusion (tool is dropped).

## Per-Category Limits

Limit how many tools from each category can appear in the final set:

```json
{
  "per_category_limits": {
    "filesystem": 3,
    "browser": 2
  }
}
```

## Forced Tools

Tools listed in `forced_tools` are always included regardless of similarity scores. Default forced tools are general-purpose utilities that the LLM should always have access to.

## Architecture

```
ToolRag
  ├── embedder: Arc<dyn EmbeddingProvider>
  ├── store: Option<Arc<MemoryStore>>
  ├── opts: ToolRagOptions
  └── specs: RwLock<HashMap<ToolName, ToolSpec>>

MemoryStore.tool_embedding_index
  ├── tool_name (TEXT)
  ├── field (TEXT: "summary" | "description" | "negative")
  ├── field_key (TEXT: "" for ToolSpec, action name for ActionSpec)
  ├── version_hash (TEXT: content-derived)
  ├── model_name (TEXT)
  └── embedding (f32 blob)
```

## Debugging

Use the CLI `/memory search` command to test tool embeddings:

```
/memory search "read a file"
```

This shows which tools match the query and their similarity scores.
