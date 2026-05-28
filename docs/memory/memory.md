# Long-Term Memory

SQLite + sqlite-vec + Diesel powered episodic memory with vector similarity search and LLM-driven summarization.

## Initialization

The `EneActor` initializes memory during `reconfigure()`:

1. Create embedding provider from `embedding` config
2. If `memory.enabled == true`, call `MemoryStore::open()`
3. Register sqlite-vec extension and run migrations
4. Attach store and embedder to `session.memory`

Memory is also available in snapshots (`EneStateSnapshot`) for CLI commands like `/memory search` and `/session summaries`.

## MemoryStore

```rust
pub struct MemoryStore {
    pool: r2d2::Pool<SqliteConnection>,
    pub embedding_dim: usize,
}
```

Uses `r2d2` connection pooling. Each operation acquires a connection from the pool.

### Database Tables

```sql
conversation_summaries (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    summary TEXT,
    embedding BLOB,     -- f32 vector as binary
    created_at TEXT,    -- RFC3339
    ended_at TEXT       -- RFC3339
)

conversation_keyfacts (
    id INTEGER PRIMARY KEY,
    card_name TEXT,
    summary_id INTEGER REFERENCES conversation_summaries(id),
    key TEXT,
    value TEXT,
    created_at TEXT
)

conversation_logs (
    id INTEGER PRIMARY KEY,
    session_id TEXT,
    card_name TEXT,
    role TEXT,          -- "user" or "assistant"
    content TEXT,
    created_at TEXT
)

tool_embeddings (
    tool_name TEXT PRIMARY KEY,
    version_hash TEXT,
    embedding BLOB,
    created_at TEXT
)
```

### Summaries

| Method | Description |
|--------|-------------|
| `open(path, dims)` | Opens persistent store, runs migrations |
| `open_in_memory(dims)` | In-memory store for testing |
| `insert_summary(id, card, summary, facts, emb, ended)` | Insert summary + key facts in transaction. Empty `value` deletes the fact. |
| `search_summaries(query_emb, card, limit, threshold)` | Cosine similarity search via `vec_distance_cosine` |
| `list_recent_summaries(card, limit)` | Most recent by `created_at DESC` |
| `delete_summary(id)` | Cascading delete (removes associated key facts) |
| `count_summaries(card)` | Count summaries for a character |

### Key Facts

| Method | Description |
|--------|-------------|
| `get_all_keyfacts(card)` | Latest value per key (`ROW_NUMBER() PARTITION BY key ORDER BY created_at DESC`) |
| `upsert_keyfact(card, key, value)` | Insert new row (latest is selected on query) |
| `delete_keyfact(card, key)` | Remove all rows for key |
| `count_keyfacts(card)` | Count distinct keys |

### Conversation Logs

| Method | Description |
|--------|-------------|
| `insert_log(id, card, role, content)` | Record a single message |
| `get_logs_by_session(id)` | Retrieve all messages for a session |

### Tool Embeddings

| Method | Description |
|--------|-------------|
| `upsert_tool_embedding(name, hash, emb)` | UPSERT tool embedding |
| `list_tool_embeddings()` | List all (name, hash, vector) |
| `delete_tool_embedding(name)` | Remove a tool's embedding |
| `search_tools(query_emb, limit, threshold)` | Cosine similarity tool search for Tool RAG |

## EmbeddingProvider

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}
```

Implementations:
- `ApiEmbeddingProvider` — OpenAI-compatible API
- `GgufEmbeddingProvider` — Local GGUF inference via Candle (GPU-free)

## Summarization

`summarize_conversation()` calls the LLM to produce a structured summary:

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

A dedicated summarization model can be configured via `memory.summarization_model` and `memory.summarization_base_url` (falls back to main LLM if empty).

## Prompt Injection Format

`format_summaries_for_prompt()` renders recalled summaries for the prompt:

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```
