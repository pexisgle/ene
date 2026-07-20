# Tool Semantic RAG Retrieval Specifications (`ene-tool-rag`)

The `ene-tool-rag` crate implements the tool RAG (Retrieval-Augmented Generation) pipeline. It dynamically selects the most relevant tools for a given user query to optimize LLM system prompt sizes.

---

## 1. Struct Definition & Main Constructors

### `ToolRag` (Public / Struct)
Coordinates vector embedding indexing and similarity searches for tools.
```rust
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
}
```

#### `new`
*   **Signature**: `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self`
*   **Description**: Constructs a `ToolRag` instance with explicit embedding providers, database connections, and weight options.

#### `from_config`
*   **Signature**: `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: crate::config::ToolRagConfig) -> Result<Self, crate::ToolRagError>`
*   **Description**: Constructs `ToolRag` using settings loaded from `config.json`.

---

## 2. Selection & Retrieval Operations (`rag.rs`)

#### `select`
*   **Signature**: `pub async fn select(&self, query: &str) -> Vec<ToolSpec>`
*   **Description**: Entry method that computes the embedding for a user query and returns matching `ToolSpec` definitions.

#### `select_with_embedding`
*   **Signature**: `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>`
*   **Process**:
    1.  Bypasses searches and returns forced tools directly if database stores are missing.
    2.  Queries the sqlite-vec backend via `MemoryStore::search_tools` to retrieve field similarities.
    3.  Groups matches by tool name and aggregates scores using `FieldWeights` multipliers.
    4.  Filters out candidates with scores below `min_similarity`.
    5.  Applies per-category limits (`per_category_limits`) to enforce diversity.
    6.  Sorts candidates and returns the top `final_n` tools, merged with forced-inject actions.

#### `forced_only_specs`
*   **Signature**: `fn forced_only_specs(&self) -> Vec<ToolSpec>`
*   **Description**: Compiles specifications for actions configured to bypass RAG filtering.

#### `stats`
*   **Signature**: `pub async fn stats(&self) -> ToolRagStats`
*   **Description**: Returns performance statistics, including total indexed tools and cached vector count.

---

## 3. Database Vector Indexing (`rag.rs`)

#### `ensure_index`
*   **Signature**: `pub async fn ensure_index(&self, specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> Result<(), EmbeddingError>`
*   **Process**:
    1.  Computes schema version hashes via `compute_index_hash` to identify changes.
    2.  If hashes match, it skips execution.
    3.  Retrieves indexed hashes from the database via `MemoryStore::list_tool_embedding_hashes` to identify stale records.
    4.  Deletes database vectors for changed or deleted tools via `MemoryStore::delete_tool_embeddings`.
    5.  Computes and persists new embeddings for missing or modified tool fields (summaries, descriptions, capabilities, examples, negative keywords) via `index_field`.

#### `index_field`
*   **Signature**: `async fn index_field(&self, store: &Arc<MemoryStore>, cached: &HashMap<(String, String, String), (String, String)>, model_name: &str, profile: &ToolRagProfile, field: EmbeddingField, field_key: &str, example_index: Option<usize>, parameters: Option<&serde_json::Value>) -> Result<(), EmbeddingError>`
*   **Description**: Formats tool fields into text prompts, calculates their embedding vectors, and saves them to the database.

#### `start_background_indexer`
*   **Signature**: `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>, profiles: Vec<ToolRagProfile>)`
*   **Description**: Spawns an asynchronous task to index tools in the background.

#### `field_version_hash`
*   **Signature**: `fn field_version_hash(field_name: &str, text: &str) -> String`
*   **Description**: Computes Blake3 hashes for field contents to track updates.

#### `is_cached`
*   **Signature**: `fn is_cached(cached: &HashMap<(String, String, String), (String, String)>, key: &(String, String, String), hash: &str, model: &str) -> bool`
*   **Description**: Checks if a field's vector is up to date in the database cache.

#### `persist`
*   **Signature**: `async fn persist(store: &Arc<MemoryStore>, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), EmbeddingError>`
*   **Description**: Saves calculated vectors back to the database.

#### `compute_index_hash`
*   **Signature**: `fn compute_index_hash(specs: &[ToolSpec], profiles: &[ToolRagProfile]) -> u64`
*   **Description**: Computes a combined hash of all tool specifications to verify index integrity.
