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
    db: DatabaseConnection, // private; sea-orm connection (sqlx-backed SQLite pool)
    embedding_dim: usize,   // private; use embedding_dim() getter
}
```

Uses `sea-orm` (backed by `sqlx`'s built-in connection pool) for async database access. Each operation acquires a connection from the pool.

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

tool_embedding_index (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_name TEXT NOT NULL,
    field TEXT NOT NULL CHECK (field IN ('summary','description','capability','example','negative')),
    field_key TEXT NOT NULL,        -- "" for ToolSpec, action name for ActionSpec
    version_hash TEXT NOT NULL,
    model_name TEXT NOT NULL,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(tool_name, field, field_key, model_name)
)

__tool_schemas (
    prefix TEXT PRIMARY KEY,    -- tool name prefix (e.g. "fs_", "utility_")
    schema_json TEXT,           -- full JSON schema declaration
    fingerprint TEXT,           -- blake3 hash of schema_json
    created_at TEXT             -- RFC3339
)
```

The `__tool_schemas` table is a metadata registry used by the Tool DB IPC server to track which tools have declared their table schemas. Tool-specific tables (e.g. `fs_undo_entries`, `utility_todo_items`) are created dynamically when tools connect and declare their schemas.

### Summaries

| Method | Description |
|--------|-------------|
| `open(path, dims)` | Opens persistent store, runs migrations |
| `open_in_memory(dims)` | In-memory store for testing |
| `insert_summary(session_id, card, summary, facts, emb, ended)` | Insert summary + key facts in transaction. Empty `value` deletes the fact. |
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

### Tool Embeddings (Multi-Vector)

Each tool has multiple embedding rows (one per field: `summary`, `description`, `capability`, `example`, `negative`). The per-field approach enables `search_tools` to aggregate relevance via max-pool across fields. The `field_key` distinguishes top-level ToolSpec embeddings from per-action ActionSpec embeddings. The `model_name` allows re-embedding with different models.

| Method | Description |
|--------|-------------|
| `upsert_tool_embedding_field(name, field, field_key, model, hash, emb)` | UPSERT a single field embedding |
| `list_tool_embedding_fields()` | List all `(name, field, field_key, model, hash, vector)` rows |
| `delete_tool_embeddings(name)` | Remove all field rows for a tool |
| `search_tools(query_emb, limit, threshold)` | Cosine similarity across all fields, max-pool per tool for Tool RAG |

## Tool DB IPC Server

Tools that need persistent storage (e.g. `ene-tool-fs` for undo, `ene-tool-utility` for todos) access the database through a per-tool IPC server rather than linking `ene-memory` directly.

### Architecture

```
Core (ene-core)                     Tool binary (e.g. ene-tool-fs)
┌─────────────────────┐             ┌──────────────────────┐
│ DbIpcServer         │  Unix sock  │ DbClient             │
│  - listens on       │◄───────────►│  - connect()         │
│    ene-db-{name}.sock│             │  - declare_schema()  │
│  - validates prefix │             │  - insert/select/... │
│  - enforces schema  │             └──────────────────────┘
│  - dispatches to    │
│    memory.db via    │
│    sea-orm          │
└─────────────────────┘
```

### Security Model

- Each tool declares its tables via `DeclareSchema` with a prefix (e.g. `fs_`, `utility_`)
- All table names must start with the tool's prefix
- All column references are validated against the declared schema
- Access to internal tables (`sqlite_*`, `__tool_schemas`, core tables) is blocked
- No DDL is exposed — tools can only use CRUD operations on their declared tables

### ene-tool-db Crate

The `ene-tool-db` crate provides:
- `DbValue` — type-safe value enum (Null/Bool/Int/Float/Text/Blob)
- `DbFilter` — structured filter expressions (Eq/Ne/Lt/Gt/In/Like/And/Or/Not/...)
- `DbSchema` / `DbTable` / `DbColumn` / `DbIndex` — schema declaration types
- `DbClient` — async client that connects to the per-tool Unix socket
- `DbRequest` / `DbResponse` — IPC message types

## EmbeddingProvider

```rust
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed text with a context kind (prefix / chunking strategy).
    async fn embed(&self, text: &str, kind: EmbeddingKind)
        -> Result<Vec<f32>, EmbeddingError>;
    /// Embed a query (default: embed(.., Query)).
    async fn embed_query(&self, text: &str)
        -> Result<Vec<f32>, EmbeddingError>;
    /// Batch embed multiple items (default: serial loop; override for performance).
    async fn embed_batch(
        &self, items: &[(&str, EmbeddingKind)],
    ) -> Result<Vec<Vec<f32>>, EmbeddingError>;
    /// Hypothetical Document Embedding — generates a synthetic document from query.
    /// Default: echo query; LLM-backed impls use cheap completion.
    async fn hyde(&self, query: &str) -> Result<String, EmbeddingError>;
    /// Rerank candidates by relevance. Default: cosine similarity;
    /// LLM-backed impls use structured output scoring.
    async fn rerank(&self, query: &str, candidates: &[ToolSpec])
        -> Result<Vec<f32>, EmbeddingError>;
    fn dimensions(&self) -> usize;
    fn model_name(&self) -> &str;
}

/// Kinds for embedding context-aware prefixes.
pub enum EmbeddingKind { Summary, Description, Capability, Example, Negative, Query, Hyde }

/// Errors from embedding operations.
pub enum EmbeddingError {
    /// Provider initialization failed (missing model file, bad API key, etc.).
    Init(String),
    /// The provider returned an error.
    Provider(String),
    /// Input was empty or whitespace-only.
    EmptyInput,
}
```

Implementations:
- `CloudEmbeddingProvider` — OpenAI-compatible API with batch embedding and optional HyDE via LLM.
- `GgufEmbeddingProvider` — Local GGUF inference via Candle (GPU-free), serial batch.
- `HybridRerankProvider` — Wraps a primary embedder with optional LLM for HyDE / rerank.

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

## Typed Memory & Memory Arbiter (Cognitive Runtime)

The cognitive runtime stores long-term facts in `typed_memories` with explicit `MemoryKind` and `MemoryStatus` lifecycle (`active`, `faded`, `archived`, `disputed`, `superseded`, `user_deleted`).

After each turn, deterministic and LLM extractors produce `MemoryCandidate` items. The **Memory Arbiter** (`ene-cognition::memory_writer::MemoryArbiter`) validates each candidate against existing memories before calling `MemoryStore::insert_typed_memory` or `MemoryStore::supersede_typed_memory`.

Key store APIs:

| Method | Description |
|--------|-------------|
| `insert_typed_memory(item)` | Insert a new typed memory row |
| `supersede_typed_memory(new_item, old_id)` | Atomically insert replacement and mark prior row `superseded` |
| `update_typed_memory_status(id, status)` | Lifecycle transition (e.g. `user_deleted`, `disputed`) |
| `search_typed_memories(embedding, ...)` | Vector similarity search over active memories |
| `search_typed_memories_hybrid(options)` | Hybrid recall with explainable score breakdown |
| `list_recallable_typed_memories(character_id, user_id, limit)` | List `active` / `faded` / `disputed` memories for recall |

See [Cognitive Runtime ADR](../architecture/cognitive-runtime.md#memory-arbiter) for decision rules and thresholds.

### Hybrid Memory Search (#73)

Typed memory recall can combine multiple signals instead of vector similarity alone. Use `MemorySearchOptions` with `MemoryStore::search_typed_memories_hybrid` to obtain `ScoredMemory` results that include a `MemoryScoreBreakdown` and the recall sources (`vector`, `lexical`, `recent`, `commitment`).

Default scoring formula:

```text
score =
  vector_similarity * 0.40
+ lexical_score     * 0.15
+ recency_score     * 0.10
+ salience          * 0.15
+ confidence        * 0.05
+ emotional_match   * 0.05
+ relationship      * 0.05
+ access_boost      * 0.05
+ commitment_boost  (active commitments only; default 0.25)
- contradiction_penalty
- stale_penalty
```

`MemorySearchOptions` also supports:

- `min_score` — drop results below this hybrid total
- `commitment_boost` — configurable boost for commitment-sourced candidates
- `recent_fallback_limit` — cap on pure-recent fallback candidates (default `5`)

Behavior:

- `Archived`, `Superseded`, and `UserDeleted` memories are excluded from normal hybrid recall.
- `Faded` and `Disputed` memories participate in recallable vector search and receive penalties when applicable.
- `Faded` and expired memories remain recallable but receive `stale_penalty`.
- Lexical candidates are gathered via token-based DB lookup across recallable rows, not only the most recently updated pool.
- Pure-recent fallback is limited; unrelated recent memories are not admitted to the full candidate pool.
- Active commitments linked via the commitment ledger are surfaced even when vector similarity is low.
- When `user_id` is set, user-specific memories from other users are excluded; character-scoped rows with an empty `user_id` remain visible.
- Candidates gathered from multiple sources are de-duplicated by memory id before ranking.

`search_typed_memories(...)` remains available as the legacy vector-only API for callers that only need cosine similarity.

## Companion Commitment Ledger

User and companion follow-ups (e.g. “next time let’s talk about X”) are stored in a dedicated `commitments` table:

```sql
commitments (
    id INTEGER PRIMARY KEY,
    character_id TEXT NOT NULL,
    user_id TEXT NOT NULL DEFAULT '',
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',  -- active | done | cancelled | stale
    due_at TEXT NULL,
    due_label TEXT NULL,                    -- raw hint from extraction ("tomorrow", "次回")
    source_memory_id INTEGER NULL REFERENCES typed_memories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT NULL
)
```

| Method | Description |
|--------|-------------|
| `insert_commitment(item)` | Insert a new commitment row |
| `get_commitment(id)` | Fetch by primary key |
| `get_commitment_by_source_memory(memory_id)` | Lookup ledger row for a typed memory |
| `list_active_commitments(character_id, user_id, limit)` | Active rows for prompt injection (no vector search) |
| `complete_commitment(id)` | Mark `done` |
| `cancel_commitment(id)` | Mark `cancelled` |
| `mark_stale_commitments(now)` | Mark overdue `active` rows as `stale` |

**Due dates:** Extractors populate `MemoryCandidate::commitment_due`, which is stored as `due_label` on the ledger row. Natural-language due-date parsing into `due_at` is not implemented yet (see Memory Arbiter notes in [Cognitive Runtime ADR](../architecture/cognitive-runtime.md#companion-commitment-ledger)), so `mark_stale_commitments` only affects rows with an explicit `due_at`.

**Runtime wiring:** `CommitmentLedger::arbitrate_apply_and_sync` runs the Memory Arbiter and syncs commitment rows in one call. `active_prompt_candidates` produces lightweight DTOs for the Active Commitments `PromptPacket` section (#87). MemoryWriter orchestration that calls sync after each turn is planned in #100. CLI list/complete commands are planned in #94.
