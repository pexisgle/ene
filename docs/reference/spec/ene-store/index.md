# `ene-store` Crate Role & DB Schema Specifications

The `ene-store` crate is Ene's isolated persistence layer. It manages SQLite database connections (`memory.db`), runs SeaORM schema migrations, and implements vector similarity searches (cosine distance) using the `sqlite-vec` extension.

---

## 1. Dependencies and Boundaries

### Physical Dependencies (`Cargo.toml`)
- **External Dependencies**: `sea-orm`, `sea-orm-migration`, `libsqlite3-sys`, `sqlite-vec`, `tokio`, `chrono`, `serde`
- **Workspace Dependencies**: `ene-config`
- **Architectural Rules**: `ene-store` must not depend on `ene-mind`, `ene-ai`, or `ene-runtime`. Decoupling the database layer prevents compilation cycles and ensures database actions remain strictly isolated.

---

## 2. Connection Settings & SQLite Pragmas

### WAL Mode and Performance Options
To optimize multi-threaded concurrent performance, the SQLite connection is configured with the following parameters:
*   `journal_mode = WAL`: Write-Ahead Logging. Ensures reading tasks do not block writing transactions.
*   `synchronous = NORMAL`: Sync state is relaxed for WAL mode, reducing disk sync I/O calls.
*   `busy_timeout = 5000`: Sets the lock timeout to 5 seconds. If locked, concurrent writers back off and retry.
*   `foreign_keys = ON`: Enforces referential integrity.

### `sqlite-vec` Registration
The `init_sqlite_vec` function registers the vector extension globally:
- **Mechanics**: Leverages SQLite's dynamic extension loading API `sqlite3_auto_extension` to bind the `sqlite-vec` binary.
- **Concurrency**: Guarded by `std::sync::Once` to prevent duplicate initialization. Provides functions like `1.0 - vec_distance_cosine(embedding, ?)` directly in SQL statements.

---

## 3. Database Schema Overview

The database Migrator generates the following tables:

```mermaid
erDiagram
    conversation_logs {
        integer id PK
        text session_id
        text role
        text content
        datetime created_at
    }
    affect_states {
        text character_id PK
        real valence
        real arousal
        real dominance
        real irritation
        real fatigue
        real affinity
        text last_expression
        text mood_label
        datetime updated_at
    }
    pending_affect_proposals {
        integer id PK
        text character_id
        text user_id
        integer source_turn_id
        real valence
        real arousal
        real irritation
        real affinity
        text recommended_expression
        real confidence
        text reason
        datetime created_at
    }
    typed_memories {
        integer id PK
        text scope
        text character_id
        text user_id
        text kind
        text title
        text content
        text source
        text source_ref
        real confidence
        real salience
        text affect
        real relationship_impact
        integer access_count
        datetime last_accessed_at
        datetime created_at
        datetime updated_at
        datetime valid_from
        datetime valid_until
        text status
        integer supersedes_id
        boolean pinned
        datetime faded_at
        integer commitment_id
    }
    memory_embeddings {
        integer memory_id PK, FK
        text model_name
        text source_text
        blob embedding
    }
    memory_links {
        integer from_id PK, FK
        integer to_id PK, FK
        text link_type
        real strength
        datetime created_at
    }
    memory_spans {
        integer id PK
        text session_id
        integer turn_start
        integer turn_end
        text raw_excerpt
        text compressed_summary
        integer compression_level
    }
    commitments {
        integer id PK
        text character_id
        text user_id
        text description
        text status
        datetime created_at
        datetime updated_at
        datetime resolved_at
        text fail_reason
    }
    tool_schemas {
        text tool_name PK
        text schema_json
        datetime declared_at
    }
    tool_embedding_index {
        integer id PK
        text tool_name FK
        text field
        text field_key
        text version_hash
        text model_name
        text source_text
        blob embedding
        datetime indexed_at
    }

    typed_memories ||--o| memory_embeddings : "id = memory_id"
    typed_memories ||--o| commitments : "commitment_id = id"
    tool_schemas ||--o{ tool_embedding_index : "tool_name"
```
*   `typed_memories`: Stores memory attributes, including status, pinning flags, and access counters.
*   `memory_embeddings`: A shadow table storing float32 embedding arrays as blobs.
*   `tool_embedding_index`: Multi-vector index for Tool RAG fields.
