# `ene-memory` — API Reference

> **Crate:** `ene-memory`  
> **Role:** Persistent vector memory store for conversation summaries, key facts, conversation logs, and tool embeddings.

---

## Overview

`ene-memory` provides Ene's long-term memory subsystem. It uses **SQLite** as the storage backend with **`sqlite-vec`** for vector similarity search and **Diesel** for all SQL access.

Each character has a separate namespace within the shared database, keyed by `card_name`. The memory system stores:

- **Summaries** — LLM-generated summaries of past sessions, with embeddings for semantic recall.
- **Key Facts** — Structured key-value facts extracted per session (e.g., user preferences, important dates).
- **Conversation Logs** — Immutable record of every message in every session.
- **Tool Embeddings** — Embeddings of tool spec fields for the tool RAG index.

> **Architecture constraint:** Tool binaries must NOT link `ene-memory` directly. They access the database through the `DbIpcServer` / `ene-tool-db` IPC client.

---

## `MemoryStore`

```rust
pub struct MemoryStore { /* opaque */ }
```

### Construction

| Method | Signature | Description |
|--------|-----------|-------------|
| `open` | `fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>` | Opens (or creates) a SQLite database at the given path with the specified embedding dimension. |
| `open_in_memory` | `fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>` | Opens an in-memory database. Primarily for testing. |
| `embedding_dim` | `fn embedding_dim(&self) -> usize` | Returns the configured embedding dimension. |

---

## Summary Methods

Summaries are the primary memory unit — each represents a completed conversation session.

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_summary` | `fn insert_summary(&self, session_id: &str, card_name: &str, summary: &str, key_facts: &[KeyFact], embedding: &[f32], ended_at: DateTime<Utc>) -> Result<i64, MemoryError>` | Inserts a new session summary with its embedding vector. Returns the row ID. |
| `search_summaries` | `fn search_summaries(&self, query_embedding: &[f32], card_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<RecalledSummary>, MemoryError>` | Searches summaries by cosine similarity. Returns at most `limit` results above `similarity_threshold`. |
| `list_recent_summaries` | `fn list_recent_summaries(&self, card_name: &str, limit: usize) -> Result<Vec<ConversationSummary>, MemoryError>` | Returns the most recent `limit` summaries sorted by `ended_at` descending. |
| `count_summaries` | `fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError>` | Returns the total number of summaries for the given character. |
| `delete_summary` | `fn delete_summary(&self, id: i64) -> Result<usize, MemoryError>` | Deletes a summary by its row ID. Returns the number of rows deleted. |

---

## Key-Fact Methods

Key facts are structured `key=value` pairs extracted from sessions and persisted per character.

| Method | Signature | Description |
|--------|-----------|-------------|
| `get_all_keyfacts` | `fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, MemoryError>` | Returns all key-facts for the given character. |
| `upsert_keyfact` | `fn upsert_keyfact(&self, card_name: &str, key: &str, value: &str) -> Result<(), MemoryError>` | Inserts or updates the fact with the given key. |
| `delete_keyfact` | `fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError>` | Deletes the fact with the given key. Returns rows deleted. |
| `count_keyfacts` | `fn count_keyfacts(&self, card_name: &str) -> Result<i64, MemoryError>` | Returns the total number of facts for the given character. |

---

## Conversation Log Methods

The conversation log is an append-only record of every user/assistant message.

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_log` | `fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>` | Inserts a log entry. Returns the row ID. |
| `spawn_insert_log` | `fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)` | Fire-and-forget log insert. Spawns a Tokio task; errors are logged but not propagated. |
| `get_logs_by_session` | `fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError>` | Returns all log entries for a session as `(role, content, created_at)` tuples. |

---

## Tool Embedding Methods

Tool embeddings power the RAG-based tool selection system.

| Method | Signature | Description |
|--------|-----------|-------------|
| `upsert_tool_embedding_field` | `fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32]) -> Result<(), MemoryError>` | Inserts or updates an embedding for a specific field of a tool spec. |
| `list_tool_embedding_fields` | `fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>` | Returns all tool embedding records (used to detect stale embeddings). |
| `delete_tool_embeddings` | `fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>` | Removes all embedding records for a tool. |
| `search_tools` | `fn search_tools(&self, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>` | Searches tools by vector similarity. Returns `(tool_name, score)` pairs. |

---

## Convenience Method

### `recall_context`

```rust
pub fn recall_context(
    &self,
    card_name: &str,
    query_embedding: &[f32],
    limit: usize,
    similarity_threshold: f32,
) -> Result<(Vec<RecalledSummary>, Vec<KeyFact>), MemoryError>
```

A single call that retrieves both relevant summaries (by vector search) and all key facts for the given character. Used by `fetch_memory_context` in `ene-core`.

---

## Global Functions

### `init_sqlite_vec`

```rust
pub fn init_sqlite_vec(conn: &mut SqliteConnection) -> Result<(), MemoryError>
```

Loads and initializes the `sqlite-vec` extension in an existing Diesel connection. Called automatically by `MemoryStore::open`.

### `format_summaries_for_prompt`

```rust
pub fn format_summaries_for_prompt(summaries: &[RecalledSummary]) -> String
```

Formats a slice of recalled summaries into a human-readable text block suitable for injection into the LLM system prompt.

### `summarize_conversation`

```rust
pub async fn summarize_conversation(
    provider: &dyn LlmProvider,
    messages: &[(Role, String)],
    card_name: &str,
    user_name: &str,
    existing_facts: &[KeyFact],
) -> Result<ConversationSummaryResult, MemoryError>
```

Calls the LLM to generate a summary and extract updated key facts from a completed session. Used internally by `execute_split` in `ene-session`.

---

## Data Types

### `ConversationSummary`

```rust
pub struct ConversationSummary {
    /// Database row ID.
    pub id: i64,

    /// The session this summary was created from.
    pub session_id: String,

    /// The character this summary belongs to.
    pub card_name: String,

    /// The LLM-generated summary text.
    pub summary: String,

    /// The embedding vector of the summary.
    pub embedding: Vec<f32>,

    /// When this summary was created.
    pub created_at: DateTime<Utc>,

    /// When the session ended.
    pub ended_at: DateTime<Utc>,
}
```

### `RecalledSummary`

```rust
pub struct RecalledSummary {
    /// The underlying summary entry.
    pub entry: ConversationSummary,

    /// Cosine similarity score against the query (0.0–1.0).
    pub similarity: f32,
}
```

### `KeyFact`

```rust
pub struct KeyFact {
    /// The fact identifier (e.g., `"user_name"`, `"favorite_color"`).
    pub key: String,

    /// The fact value.
    pub value: String,
}
```

### `ConversationSummaryResult`

```rust
pub struct ConversationSummaryResult {
    /// The generated summary text.
    pub summary: String,

    /// Updated or newly extracted key-facts.
    pub key_facts: Vec<KeyFact>,
}
```

---

## Database Architecture

| Layer | Technology |
|-------|------------|
| SQL ORM | [`sea-orm`](https://www.sea-ql.org/SeaORM) (with `sqlx-sqlite` feature) |
| Connection pooling | [`sqlx::Pool`](https://docs.rs/sqlx) (built into the `sqlx-sqlite` backend) |
| Vector search | [`sqlite-vec`](https://github.com/asg017/sqlite-vec) (loaded as SQLite extension) |
| Storage backend | SQLite (single file per user profile) |

> **Rule:** Always use `sea-orm` (and `sea-orm-migration`) for all SQL in this crate. Do **not** introduce `rusqlite` or `diesel` — this violates the project constraint (§7.3 of AGENTS.md).

Migrations are defined as Rust modules under `crates/ene-memory/src/migrator/src/m{YYYYMMDD}_{name}/` (scaffold via the `sea-orm-migration` CLI) and embedded through `Migrator` re-exports.

---

## Usage Example

```rust
use ene_memory::{MemoryStore, KeyFact};
use std::path::Path;

// Open (or create) the database
let store = MemoryStore::open(Path::new("data/memory.db"), 1024)?;

// Upsert some facts about the user
store.upsert_keyfact("alice", "favorite_color", "blue")?;
store.upsert_keyfact("alice", "city", "Tokyo")?;

// Recall context for a query
let query_vec: Vec<f32> = vec![/* ... embedding ... */];
let (summaries, facts) = store.recall_context("alice", &query_vec, 5, 0.7)?;

println!("Recalled {} summaries and {} facts", summaries.len(), facts.len());
for s in &summaries {
    println!("[{:.2}] {}", s.similarity, s.entry.summary);
}
for f in &facts {
    println!("  {}: {}", f.key, f.value);
}
```

---

## See Also

- [`ene-session`](./ene-session.md) — Drives session splits that create summaries
- [`ene-core`](./ene-core.md) — `MemoryQueryHandle` for external access
- [`ene-embedding`](./ene-embedding.md) — Provides embeddings for storage and search
