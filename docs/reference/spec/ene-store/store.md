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

#### `init_sqlite_vec`
*   **Signature**: `pub fn init_sqlite_vec()`
*   **Description**: Registers the `sqlite-vec` binary extension globally via SQLite auto-extension hooks (`sqlite3_auto_extension`). Guarded by a `Once` block to prevent duplicate registry errors.

#### `open`
*   **Signature**: `pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>`
*   **Process**:
    1.  Invokes `init_sqlite_vec` to verify vector capabilities.
    2.  Creates target connection options under the `sqlite://` scheme.
    3.  Applies WAL mode pragmas, synchronous NORMAL, and busy timeouts.
    4.  Runs schema migrations (`Migrator::up(&db, None)`).
    5.  Returns the instantiated `MemoryStore`.

#### `open_in_memory`
*   **Signature**: `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>`
*   **Description**: Connects to an in-memory SQLite store (`sqlite::memory:`) for test runs.

#### `connection`
*   **Signature**: `pub const fn connection(&self) -> &DatabaseConnection`
*   **Description**: Exposes the underlying SeaORM database connection handle.

#### `embedding_dim`
*   **Signature**: `pub const fn embedding_dim(&self) -> usize`
*   **Description**: Returns the dimension size of configured embedding models.

#### `apply_pragmas`
*   **Signature**: `async fn apply_pragmas(db: &DatabaseConnection) -> Result<(), MemoryError>`
*   **Description**: Configures WAL journal modes, synchronous NORMAL, busy timeout thresholds (5000ms), and enables foreign key constraints.

---

## 2. Text Logs and Summaries

#### `insert_log`
*   **Signature**: `pub async fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>`
*   **Description**: Inserts a raw dialog message line into the `conversation_logs` table.

#### `insert_conversation_turn`
*   **Signature**: `pub async fn insert_conversation_turn(&self, session_id: &str, card_name: &str, user_message: &str, assistant_response: &str) -> Result<(i64, i64), MemoryError>`
*   **Description**: Appends both user and assistant exchange messages in a single transaction, returning their log database primary keys.

#### `spawn_insert_log`
*   **Signature**: `pub fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)`
*   **Description**: Spawns an asynchronous log writer task onto the runtime, preventing write latency from delaying active chat threads.

#### `get_logs_by_session`
*   **Signature**: `pub async fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError>`
*   **Description**: Queries the chronological transcript logs (roles, content, timestamps) associated with a target session UUID.

---

## 3. Tool Embedding Index Operations (Tool RAG)

#### `upsert_tool_embedding_field`
*   **Signature**: `pub async fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), MemoryError>`
*   **Description**: Updates or inserts vector embeddings for tool specification fields (such as capability lists and example patterns).

#### `list_tool_embedding_fields`
*   **Signature**: `pub async fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>`
*   **Description**: Lists all indexed tool specifications.

#### `list_tool_embedding_hashes`
*   **Signature**: `pub async fn list_tool_embedding_hashes(&self) -> Result<Vec<(String, String, String, String, String)>, MemoryError>`
*   **Description**: Helper returning indexed tool hashes (names, keys, versions) to identify stale vector segments.

#### `delete_tool_embeddings`
*   **Signature**: `pub async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>`
*   **Description**: Deletes all indexed embeddings associated with an external tool name.

#### `search_tools`
*   **Signature**: `pub async fn search_tools(&self, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>`
*   **Description**: Queries the `tool_embedding_index` for tool names whose specifications match the query embedding vector using cosine distance filters.

---

## 4. Emotional State & Proposals

#### `get_affect_state`
*   **Signature**: `pub async fn get_affect_state(&self, character_id: &str) -> Result<crate::AffectState, MemoryError>`
*   **Description**: Queries SQLite for the character's PAD ( Valence, Arousal, Irritation, Fatigue, Affinity ) state. Returns a neutral baseline if no record exists.

#### `upsert_affect_state`
*   **Signature**: `pub async fn upsert_affect_state(&self, state: &crate::AffectState) -> Result<(), MemoryError>`
*   **Description**: Persists the updated emotional state coordinates and mood labels back to the database.

#### `upsert_pending_affect_proposal`
*   **Signature**: `pub async fn upsert_pending_affect_proposal(&self, proposal: &crate::PendingAffectProposal) -> Result<(), MemoryError>`
*   **Description**: Saves proposals compiled from post-turn background classifiers into the `pending_affect_proposals` table.

#### `get_pending_affect_proposal`
*   **Signature**: `pub async fn get_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<Option<crate::PendingAffectProposal>, MemoryError>`
*   **Description**: Fetches the staged emotional proposal waiting to be applied at the start of the next turn.

#### `delete_pending_affect_proposal`
*   **Signature**: `pub async fn delete_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<(), MemoryError>`
*   **Description**: Clears pending emotional state proposals.

#### `take_pending_affect_proposal`
*   **Signature**: `pub async fn take_pending_affect_proposal(&self, character_id: &str, user_id: &str) -> Result<Option<crate::PendingAffectProposal>, MemoryError>`
*   **Description**: Fetches and deletes pending emotional state proposals in a single transaction.

---

## 5. Long-Term Memory CRUD Operations

#### `insert_typed_memory`
*   **Signature**: `pub async fn insert_typed_memory(&self, item: &crate::NewMemoryItem) -> Result<i64, MemoryError>`
*   **Description**: Inserts a memory record and its corresponding vector embedding into `typed_memories` and `memory_embeddings` respectively inside a single transaction.

#### `get_typed_memory`
*   **Signature**: `pub async fn get_typed_memory(&self, id: i64) -> Result<Option<crate::MemoryItem>, MemoryError>`
*   **Description**: Fetches a single long-term memory item by ID.

#### `get_typed_memories_by_character`
*   **Signature**: `pub async fn get_typed_memories_by_character(&self, character_id: &str, kind: Option<crate::MemoryKind>, limit: usize, offset: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: Queries long-term memories associated with a character, filtered by category.

#### `count_typed_memories`
*   **Signature**: `pub async fn count_typed_memories(&self, character_id: &str, kind: Option<crate::MemoryKind>) -> Result<i64, MemoryError>`
*   **Description**: Returns counts of memory records.

#### `list_typed_memories_by_source_prefix`
*   **Signature**: `pub async fn list_typed_memories_by_source_prefix(&self, character_id: &str, prefix: &str, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: Searches memory items matching specific source namespaces.

#### `typed_memory_exists_by_source_ref`
*   **Signature**: `pub async fn typed_memory_exists_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<bool, MemoryError>`
*   **Description**: Verifies if a specific source reference ID already exists.

#### `get_active_typed_memory_by_source_ref`
*   **Signature**: `pub async fn get_active_typed_memory_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<Option<crate::MemoryItem>, MemoryError>`
*   **Description**: Retrieves active memories matching a target reference.

#### `archive_typed_memories_by_source_prefixes`
*   **Signature**: `pub async fn archive_typed_memories_by_source_prefixes(&self, character_id: &str, prefixes: &[&str], keep_refs: &std::collections::HashSet<String>) -> Result<usize, MemoryError>`
*   **Description**: Transitions unmatched memory items belonging to target namespaces to `Archived`.

#### `supersede_typed_memory`
*   **Signature**: `pub async fn supersede_typed_memory(&self, new_item: &crate::NewMemoryItem, superseded_id: i64) -> Result<i64, MemoryError>`
*   **Description**: Inserts a new memory and marks the overridden record as `Superseded` in a single transaction.

#### `update_typed_memory_status`
*   **Signature**: `pub async fn update_typed_memory_status(&self, id: i64, new_status: crate::MemoryStatus) -> Result<bool, MemoryError>`
*   **Description**: Directly updates a memory record's status.

#### `bump_typed_memory_access`
*   **Signature**: `pub async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryError>`
*   **Description**: Increments the access counter and updates `last_accessed_at` timestamps.

#### `transition_typed_memory_status`
*   **Signature**: `pub async fn transition_typed_memory_status(&self, id: i64, new_status: crate::MemoryStatus) -> Result<bool, MemoryError>`
*   **Description**: Validates and updates a memory record's status.

#### `user_restore_typed_memory`
*   **Signature**: `pub async fn user_restore_typed_memory(&self, id: i64) -> Result<bool, MemoryError>`
*   **Description**: Restores archived/faded memories back to `Active` status.

#### `user_forget_typed_memory`
*   **Signature**: `pub async fn user_forget_typed_memory(&self, id: i64) -> Result<bool, MemoryError>`
*   **Description**: Moves active memories to `UserDeleted` status.

#### `apply_natural_decay_batch`
*   **Signature**: `pub async fn apply_natural_decay_batch(&self, character_id: &str, user_id: Option<&str>, now: DateTime<Utc>, decay_half_life_days: f64, limit: usize) -> Result<NaturalDecayReport, MemoryError>`
*   **Description**: Decay calculation helper:
    1.  Calculates the time elapsed since the memory was last accessed.
    2.  Computes decay scores based on the half-life.
    3.  Sets the status of active memories to `Faded` if they drop below `0.3`.
    4.  Sets the status of faded memories to `Archived` if they drop below `0.1`.
    5.  Processes updates in batches.

---

## 6. Hybrid Retrieval Search Execution

#### `search`
*   **Signature**: `pub async fn search(&self, query: &crate::Query<'_>) -> Result<Vec<crate::ScoredMemory>, MemoryError>`
*   **Process**:
    1.  Performs vector similarity queries on `memory_embeddings` to fetch candidates.
    2.  Performs token lexical matching on `typed_memories` to fetch candidates.
    3.  Merges and scores candidates using access frequency, emotional alignment, and recency coefficients.
    4.  Returns the ranked list.

#### `search_typed_memories`
*   **Signature**: `pub(crate) async fn search_typed_memories(&self, query_embedding: &[f32], character_id: &str, model_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError>`
*   **Description**: Performs vector searches across all active memories.

#### `search_typed_memories_vector`
*   **Signature**: `async fn search_typed_memories_vector(&self, query_embedding: &[f32], character_id: &str, model_name: &str, user_id: Option<&str>, statuses: &[&str], limit: usize, similarity_threshold: f32) -> Result<Vec<(crate::MemoryItem, f32)>, MemoryError>`
*   **Description**: Base vector search executor using sqlite-vec cosine distance filters.

#### `list_recallable_typed_memories`
*   **Signature**: `pub async fn list_recallable_typed_memories(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: Lists candidate memories eligible for turn recall.

#### `get_typed_memories_by_commitment_ids`
*   **Signature**: `async fn get_typed_memories_by_commitment_ids(&self, commitment_ids: &[i64]) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: Retrieves memories linked to target commitment IDs.

#### `list_lexical_typed_memory_candidates`
*   **Signature**: `async fn list_lexical_typed_memory_candidates(&self, query_text: &str, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<crate::MemoryItem>, MemoryError>`
*   **Description**: Runs substring queries on text contents.

---

## 7. Vector Validation & Conversion Helpers

#### `validate_embedding`
*   **Signature**: `fn validate_embedding(embedding: &[f32], expected_dim: usize) -> Result<(), MemoryError>`
*   **Description**: Verifies that vector sizes match and containing floats are finite.

#### `decode_embedding_bytes`
*   **Signature**: `pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32>`
*   **Description**: Translates database binary blobs back to float32 vectors.

#### `embedding_to_bytes`
*   **Signature**: `fn embedding_to_bytes(v: &[f32]) -> Vec<u8>`
*   **Description**: Converts vector floats into binary blobs.

#### `bytes_to_embedding`
*   **Signature**: `fn bytes_to_embedding(b: &[u8]) -> Vec<f32>`
*   **Description**: Converts binary blobs back to vector floats.

#### `cosine_similarity_expr`
*   **Signature**: `fn cosine_similarity_expr(embedding_col: &str, query_bytes: &[u8]) -> sea_orm::sea_query::Expr`
*   **Description**: Compiles SQLite `vec_distance_cosine` functions into SeaORM query expressions.

#### `cosine_similarity_filter`
*   **Signature**: `fn cosine_similarity_filter(embedding_col: &str, query_bytes: &[u8], threshold: f64) -> sea_orm::sea_query::Expr`
*   **Description**: Returns filtering expressions checking cosine thresholds.
