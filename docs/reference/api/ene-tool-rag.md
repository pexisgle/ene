# ene-tool-rag — Tool RAG Pipeline

Tool RAG (Retrieval-Augmented Generation) pipeline for dynamic tool selection. When the number of available tools exceeds the LLM's context budget, `ToolRag` runs a retrieval-augmented selection step: embed → weighted multi-field similarity → embedding cosine rerank → top-N.

**Dependencies**: `ene-ai` (embedding providers), `ene-store` (persistent tool embedding storage), `ene-plugin-proto` (wire types), `ene-config`.

---

## `ToolRag`

```rust
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    last_specs_hash: AtomicU64,
    cached_field_rows: RwLock<Vec<CachedFieldRow>>,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self` | Direct constructor when you already have a resolved `ToolRagOptions`. |
| `from_config` | `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: ToolRagConfig) -> Result<Self, ToolRagError>` | Builds `ToolRagOptions` from `ToolRagConfig` (converting `Vec<String>` → `Vec<ToolName>` for `forced`) and constructs the pipeline. |
| `ensure_index` | `pub async fn ensure_index(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> Result<(), EmbeddingError>` | Computes a BLAKE3 hash over specs + profiles; if unchanged since the last call, this is a fast no-op. Otherwise (re-)embeds and stores per-field vectors (`summary`, `description`, `capability`, `example`, `negative`) from each `ToolRagProfile`. |
| `select` | `pub async fn select(&self, query: &str) -> Vec<ToolSpec>` | Embeds `query` internally, then delegates to `select_with_embedding`. |
| `select_with_embedding` | `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>` | Runs weighted per-field similarity scoring (using `query_embedding`), per-category limits, `top_k` cut, embedding cosine rerank when multiple candidates remain, and returns the top `opts.final_n` tools above `opts.min_similarity`, always including any `opts.forced` tools. |
| `start_background_indexer` | `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>, profiles: Vec<ToolRagProfile>)` | Spawns a background task that calls `ensure_index` to warm the cache; returns immediately. |
| `stats` | `pub async fn stats(&self) -> ToolRagStats` | Snapshot of the last `select`/`select_with_embedding` call: hit count, index size, and top similarity. |
| `opts` | `pub fn opts(&self) -> &ToolRagOptions` | Returns the resolved options. |
| `has_store` | `pub fn has_store(&self) -> bool` | Whether a backing `MemoryStore` is attached (RAG requires one to persist embeddings across restarts). |

---

## `ToolRagOptions`

```rust
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    pub top_k: usize,
    pub final_n: usize,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub forced: Vec<ToolName>,
    pub weights: FieldWeights,
    pub per_category_limits: HashMap<String, usize>,
}
```

Implements `TryFrom<ToolRagConfig>` — validates that all forced tool names are valid `ToolName`s.

---

## `FieldWeights`

Controls how strongly each embedding field contributes to a tool's relevance score. Negative weights (e.g. on `negative`) act as a soft penalty rather than a hard exclusion.

```rust
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub summary: f32,
    pub description: f32,
    pub capability: f32,
    pub example: f32,
    pub negative: f32,
}
```

Implements `From<FieldWeightsConfig>` for conversion from the serializable config type.

---

## `ToolRagStats`

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolRagStats {
    pub hits: usize,
    pub total: usize,
    pub top_similarity: f32,
}
```

---

## Configuration Types

`ToolRagConfig` is serialized under `tools.rag` in `settings.json` (see [Settings](../configuration/settings.md#toolsrag--tool-rag-pipeline)).

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,       // deprecated; LLM HyDE disabled (no-op, removal planned)
    pub use_rerank: bool,     // cosine embedding rerank when true
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<String>,
    pub weights: FieldWeightsConfig,
    pub per_category_limits: HashMap<String, usize>,
}
```

### `FieldWeightsConfig`

```rust
pub struct FieldWeightsConfig {
    pub summary: f32,
    pub description: f32,
    pub capability: f32,
    pub example: f32,
    pub negative: f32,
    pub hyde: f32,            // deprecated; unused
    pub hyde_blend: f32,      // deprecated; unused
}
```

Serializable counterpart of `FieldWeights` — `impl From<FieldWeightsConfig> for FieldWeights` converts between them.

---

## Errors: `ToolRagError`

```rust
#[derive(Debug, Error)]
pub enum ToolRagError {
    #[error("Tool RAG configuration error: {message}")]
    Config { message: String },
}
```

`ToolRag::from_config` / `ToolRagOptions::from_config` **fail** on invalid forced names (startup hard-error). Embed or store failures return forced tools only.

---

## Usage

Constructed by `ene-runtime` during `EneHandle::open` when tools and an embedder are available:

```rust
let rag_config = ToolRagConfig::default();
let opts = ToolRagOptions::try_from(rag_config)?;
let rag = Arc::new(ToolRag::new(embedder.clone(), store, opts));
```

Used by the streaming engine to select relevant tools before LLM inference:

```rust
let tools = match &tool_rag {
    Some(rag) => rag.select(user_input).await,
    None => registry.list_tools(),
};
```

---

## See Also

- [`ene-plugin-host`](./ene-plugin-host.md) — Tool process lifecycle manager (RAG consumer)
- [`ene-ai`](./ene-ai.md) — Embedding providers used by the pipeline
- [`ene-store`](./ene-store.md) — Persistent embedding storage (`tool_embedding_index` table)
- [`ene-plugin-proto`](./ene-plugin-proto.md) — `ToolSpec`, `ToolName`, `EmbeddingField` types
- [`ene-config`](./ene-config.md) — Configuration loading
- [Tool System Overview](../tools/overview.md)
