# `MemoryStore` / SQLite Connection & DB Operations Spec

The `MemoryStore` struct holds the SQLite (sqlite-vec) connection pool, managing chat history logs, long-term typed memories, and embedding vector CRUD transactions.

---

## 1. Struct Definition & Instantiation

### `MemoryStore` (Public / Struct)
```rust
#[derive(Clone)]
pub struct MemoryStore {
    db: DatabaseConnection,
    embedding_dim: usize,
}
```
*   `db`: SeaORM database connection pool handle.
*   `embedding_dim`: Vector size of the active embedding model (e.g. `1536` for `text-embedding-3-small`).

### Constructors

#### `open`
*   **Signature**: `pub async fn open(db_path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>`
*   **Process**:
    1.  Registers the `sqlite-vec` dynamic extension via `init_sqlite_vec()`.
    2.  Assembles the `sqlite:PATH` target connection string.
    3.  Sets WAL mode, NORMAL synchronous mode, and a 5-second busy timeout before opening `Database::connect`.
    4.  Runs `Migrator::up(&db, None)` to set up tables.

#### `open_in_memory`
*   **Signature**: `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>`
*   **Process**: Establishes a temporary connection using `sqlite::memory:` for unit and integration testing.

---

## 2. Conversation Logs and Summaries

### `insert_conversation_log`
*   **Signature**: `pub async fn insert_conversation_log(&self, session_id: &str, role: Role, content: &str) -> Result<i64, MemoryError>`
*   **Description**: Appends a message entry to the `conversation_logs` table.

### `insert_memory_span`
*   **Signature**: `pub async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryError>`
*   **Description**: Writes session split details and summary strings to the `memory_spans` table.

### `get_active_scene_summary`
*   **Signature**: `pub async fn get_active_scene_summary(&self, session_id: &str) -> Result<Option<ActiveSceneSummaryRow>, MemoryError>`
*   **Description**: Fetches the most recent scene-level (compression level 0) summary block for prompt packing.

---

## 3. Vector Validation

Before writing embeddings to sqlite, `validate_embedding` runs structural checks:
```rust
fn validate_embedding(embedding: &[f32], expected_dim: usize) -> Result<(), MemoryError>
```
*   **Dimension Match**: Asserts that the vector length matches `embedding_dim`. Dimension mismatches poison cosine calculations.
*   **Finite Check**: Rejects arrays containing `NaN` or `Infinity` components. Under sqlite-vec, a single NaN value poisons the entire cosine distance calculation, causing database queries to return empty results.

---

## 4. Natural Forgetting Batch Update

### `apply_natural_decay_batch`
*   **Signature**:
    ```rust
    pub async fn apply_natural_decay_batch(
        &self,
        character_id: &str,
        user_id: Option<&str>,
        now: DateTime<Utc>,
        decay_half_life_days: f64,
        limit: usize,
    ) -> Result<NaturalDecayReport, MemoryError>
    ```
*   **Process**:
    1.  Calculates elapsed time between the memory's last access date and `now`.
    2.  Computes the current recall score under SQL using the half-life factor.
    3.  Updates the status of `Active` memories to `Faded` if scores drop below `0.3`.
    4.  Updates the status of `Faded` memories to `Archived` if scores drop below `0.1`.
    5.  Limits updates to a batch size of `limit` (defaulting to 256) and returns the modified counts.
