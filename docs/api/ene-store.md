# `ene-store` — API Reference

> **Crate:** `ene-store`
> **Role:** Persistent long-term memory store — legacy summaries/key-facts/logs, typed memory (episodic/semantic/affective/etc.), affect state, companion commitments, and the tool-embedding index.

---

## Overview

`ene-store` is Ene's long-term memory subsystem. It uses **SQLite** as the storage backend, **`sea-orm`** (async, `sqlx-sqlite` backed) for all SQL access, and **`sqlite-vec`** for cosine-similarity vector search.

> **Architecture constraint:** This crate uses `sea-orm` + `sqlite-vec` for **all** database access. It does **not** use Diesel or raw `rusqlite`. Tool binaries must not link `ene-store` directly — they access the database through the `DbIpcServer` / `ene-tool-db` IPC client instead.

Each character has a separate namespace within the shared database, keyed by `card_name` (legacy tables) or `character_id` (typed memory / affect / commitments). The crate stores several kinds of data, layered from oldest to newest:

| Layer | Tables | Status |
|---|---|---|
| **Legacy** | `conversation_summaries`, `conversation_keyfacts`, `conversation_logs` | Read/write by default; becomes read-only once the cognitive runtime migrates a card |
| **Typed memory** | `typed_memories`, `memory_embeddings` | Primary store for the mind runtime |
| **Affect** | `affect_states` | Per-character PAD (pleasure/arousal/dominance) emotional state |
| **Commitments** | `commitments` | Companion promises / follow-ups ledger |
| **Memory spans** | `memory_spans` | Rolling scene/chapter compression over raw logs |
| **Tool index** | `tool_embedding_index` | Multi-vector embeddings for the Tool RAG pipeline |

Almost every `MemoryStore` method is `async` (it acquires a connection from the `sea-orm`/`sqlx` pool). The only **synchronous** methods are `spawn_insert_log`, `connection`, `embedding_dim`, `decode_embedding_bytes`, `legacy_write_mode`, and `set_legacy_write_mode`.

---

## Initialization

### `init_sqlite_vec`

```rust
pub fn init_sqlite_vec()
```

Registers the `sqlite-vec` extension **process-globally** via `sqlite3_auto_extension`, guarded by a `std::sync::Once` so it only runs once per process. Takes no arguments and returns nothing — it cannot fail at the API level (a failed registration would surface later as a SQL error from `vec_distance_cosine`). It is called automatically inside `MemoryStore::open` and `MemoryStore::open_in_memory`; you do not need to call it yourself.

### `MemoryStore::open` / `open_in_memory`

```rust
pub struct MemoryStore { /* opaque */ }

impl MemoryStore {
    pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>;
    pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>;
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `open` | `pub async fn open(path: &Path, embedding_dim: usize) -> Result<Self, MemoryError>` | Opens (or creates) a SQLite database file, registers `sqlite-vec`, and runs pending `sea-orm-migration` migrations. |
| `open_in_memory` | `pub async fn open_in_memory(embedding_dim: usize) -> Result<Self, MemoryError>` | Opens an in-memory SQLite database. Used in tests and the doc example below. |
| `connection` | `pub fn connection(&self) -> &DatabaseConnection` | Returns the underlying `sea_orm::DatabaseConnection`, for advanced callers that need raw query access. |
| `embedding_dim` | `pub fn embedding_dim(&self) -> usize` | Returns the configured embedding vector width. |
| `decode_embedding_bytes` | `pub fn decode_embedding_bytes(&self, bytes: &[u8]) -> Vec<f32>` | Decodes a raw `BLOB` column into an `f32` vector. Useful when working with rows fetched outside the typed store methods. |
| `legacy_write_mode` | `pub fn legacy_write_mode(&self) -> LegacyWriteMode` | Returns whether legacy-table writes are currently allowed. |
| `set_legacy_write_mode` | `pub fn set_legacy_write_mode(&self, mode: LegacyWriteMode)` | Sets the legacy write policy (see below). |

### `LegacyWriteMode`

```rust
/// Legacy table write policy for cognitive runtime integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LegacyWriteMode {
    /// Legacy summaries/keyfacts/logs may be written (default).
    #[default]
    ReadWrite,
    /// Legacy writes rejected; tables remain for read-only recall.
    ReadOnly,
}
```

When set to `ReadOnly`, `insert_summary` and `upsert_keyfact` return `MemoryError::LegacyWriteForbidden`. This is used once a character card has been migrated to typed memory (§ Legacy Migration), so nothing accidentally writes to the deprecated tables again. `insert_log` and `insert_conversation_turn` are **not** gated — raw conversation logging always continues, since it also feeds `memory_spans`.

---

## Legacy Memory: Summaries, Key Facts, Logs

The original (pre-typed-memory) memory model: one **summary** per completed session, a flat **key-fact** table per character, and an append-only **conversation log**.

### Core types

```rust
pub struct KeyFact {
    pub key: String,
    pub value: String,
}

pub struct ConversationSummary {
    pub id: i64,
    pub session_id: String,
    pub card_name: String,
    pub summary: String,
    pub embedding: Vec<f32>,
    pub created_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

pub struct RecalledSummary {
    pub entry: ConversationSummary,
    /// Cosine similarity, range `[-1.0, 1.0]` (raw angular distance, not normalized).
    pub similarity: f32,
}
```

### Summary methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_summary` | `async fn insert_summary(&self, session_id: &str, card_name: &str, summary: &str, key_facts: &[KeyFact], embedding: &[f32], ended_at: DateTime<Utc>) -> Result<i64, MemoryError>` | Inserts a summary and its key facts in one transaction. Rejected with `LegacyWriteForbidden` in `ReadOnly` mode. |
| `search_summaries` | `async fn search_summaries(&self, query_embedding: &[f32], card_name: &str, limit: usize, similarity_threshold: f32) -> Result<Vec<RecalledSummary>, MemoryError>` | Cosine-similarity search via `vec_distance_cosine`. |
| `list_recent_summaries` | `async fn list_recent_summaries(&self, card_name: &str, limit: usize) -> Result<Vec<ConversationSummary>, MemoryError>` | Most recent summaries by `created_at DESC`. |
| `count_summaries` | `async fn count_summaries(&self, card_name: &str) -> Result<i64, MemoryError>` | Total summaries for a character. |
| `delete_summary` | `async fn delete_summary(&self, id: i64) -> Result<usize, MemoryError>` | Cascading delete (removes associated key facts too). |
| `recall_context` | `async fn recall_context(&self, card_name: &str, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<(Vec<RecalledSummary>, Vec<KeyFact>), MemoryError>` | Convenience call combining a summary search with all key facts. Returns empty vectors once the card has been migrated to typed memory. |

### Key-fact methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `get_all_keyfacts` | `async fn get_all_keyfacts(&self, card_name: &str) -> Result<Vec<KeyFact>, MemoryError>` | Latest value per key. |
| `upsert_keyfact` | `async fn upsert_keyfact(&self, card_name: &str, key: &str, value: &str) -> Result<(), MemoryError>` | Inserts a new row for the key (latest row wins on read). Rejected in `ReadOnly` mode. |
| `delete_keyfact` | `async fn delete_keyfact(&self, card_name: &str, key: &str) -> Result<usize, MemoryError>` | Removes all rows for the key. |
| `count_keyfacts` | `async fn count_keyfacts(&self, card_name: &str) -> Result<i64, MemoryError>` | Number of distinct keys for a character. |

### Conversation log methods

There is no plain `get_logs` — the accessor is `get_logs_by_session`, and it returns `DateTime<Utc>` timestamps (not RFC3339 strings):

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_log` | `async fn insert_log(&self, session_id: &str, card_name: &str, role: &str, content: &str) -> Result<i64, MemoryError>` | Appends one log entry. Never gated by `LegacyWriteMode`. |
| `insert_conversation_turn` | `async fn insert_conversation_turn(&self, session_id: &str, card_name: &str, user_message: &str, assistant_response: &str) -> Result<(i64, i64), MemoryError>` | Convenience: inserts a user log entry and an assistant log entry as one call. Returns both row IDs. |
| `spawn_insert_log` | `fn spawn_insert_log(store: &Arc<Self>, session_id: &str, card_name: &str, role: &str, content: &str)` | **Synchronous.** Fire-and-forget: spawns a `tokio` task that calls `insert_log`; errors are logged, not propagated. |
| `get_logs_by_session` | `async fn get_logs_by_session(&self, session_id: &str) -> Result<Vec<(String, String, DateTime<Utc>)>, MemoryError>` | All `(role, content, created_at)` tuples for a session, ascending by `created_at`. |

---

## Tool Embeddings

Powers the Tool RAG pipeline in `ene-tool-host`. Unlike the legacy/typed memory tables, each tool spec is embedded as **multiple rows** — one per field (`summary`, `description`, `capability`, `example`, `negative`) — enabling per-field weighting and max-pool aggregation at query time.

```rust
/// `(tool_name, field, field_key, version_hash, model_name, embedding_vec, source_text)`
pub type ToolEmbeddingFieldRow = (String, String, String, String, String, Vec<f32>, String);
```

`field_key` disambiguates multiple rows sharing the same `field` (for example `"ex_0"`, `"ex_1"` for multiple usage examples on one tool). `source_text` stores the exact text that produced the embedding, so re-embedding can be skipped when the text is unchanged.

| Method | Signature | Description |
|--------|-----------|-------------|
| `upsert_tool_embedding_field` | `async fn upsert_tool_embedding_field(&self, tool_name: &str, field: &str, field_key: &str, version_hash: &str, model_name: &str, embedding: &[f32], source_text: &str) -> Result<(), MemoryError>` | Upserts on `(tool_name, field, field_key, model_name)`. |
| `list_tool_embedding_fields` | `async fn list_tool_embedding_fields(&self) -> Result<Vec<ToolEmbeddingFieldRow>, MemoryError>` | Full rows, including vectors and `source_text`. Used to rebuild the in-memory RAG index. |
| `list_tool_embedding_hashes` | `async fn list_tool_embedding_hashes(&self) -> Result<Vec<(String, String, String, String, String)>, MemoryError>` | Lightweight `(tool_name, field, field_key, version_hash, model_name)` rows (no vectors) — used to detect which tools need re-embedding. |
| `delete_tool_embeddings` | `async fn delete_tool_embeddings(&self, tool_name: &str) -> Result<usize, MemoryError>` | Removes every field row for a tool. |
| `search_tools` | `async fn search_tools(&self, query_embedding: &[f32], limit: usize, similarity_threshold: f32) -> Result<Vec<(String, f32)>, MemoryError>` | Cosine similarity across all fields, **max-pooled per tool**, sorted descending. Returns `(tool_name, score)`. |

---

## Typed Memory

The typed memory model is the primary store for the mind runtime. Each row has a `MemoryKind`, `MemoryStatus`, `MemorySource`, and independent confidence/salience scores, replacing the flat legacy summary/key-fact model with something queryable and lifecycle-aware.

### `MemoryKind`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Specific events or conversations (what happened when).
    Episodic,
    /// Facts and general knowledge (what is true).
    Semantic,
    /// Information about the user's identity, background, and traits.
    UserProfile,
    /// Information about the relationship between the user and the companion.
    Relationship,
    /// Memories with strong emotional salience.
    Affective,
    /// Promises, tasks, and obligations the companion has made.
    Commitment,
    /// User likes, dislikes, and preferences.
    Preference,
    /// How-to knowledge and procedural instructions.
    Procedure,
    /// Self-reflections by the companion about past interactions.
    Reflection,
}
```

### `MemoryStatus`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Currently relevant and retrievable.
    Active,
    /// Decayed but still retrievable with lower priority.
    Faded,
    /// No longer shown in normal recall but preserved.
    Archived,
    /// User has disputed or corrected this memory.
    Disputed,
    /// Replaced by a newer, conflicting memory.
    Superseded,
    /// Explicitly deleted by the user.
    UserDeleted,
}
```

See [Memory Forgetting Lifecycle](#forgetting-lifecycle) for the allowed transitions between statuses.

### Supporting value types

```rust
pub enum MemoryScope { Character, User, Shared }

pub enum MemorySource {
    Conversation, UserStated, LlmExtracted, Inferred, Imported, Ccv3,
}

/// Clamped to `[0.0, 1.0]`.
pub struct MemoryConfidence(f32);
/// Clamped to `[0.0, 1.0]`.
pub struct MemorySalience(f32);

pub struct AffectAnnotation {
    /// Pleasure–displeasure (-1.0..=1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0..=1.0).
    pub arousal: f32,
}
```

### `MemoryItem` / `NewMemoryItem`

```rust
pub struct MemoryItem {
    pub id: Option<i64>,
    pub scope: MemoryScope,
    pub character_id: String,
    pub user_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub source: MemorySource,
    pub source_ref: Option<String>,
    pub confidence: MemoryConfidence,
    pub salience: MemorySalience,
    pub affect: AffectAnnotation,
    pub relationship_impact: f32,
    pub access_count: i64,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: MemoryStatus,
    /// Predecessor link, set only on the successor row of a supersede operation.
    pub supersedes_id: Option<i64>,
    /// Pinned memories are exempt from natural decay.
    pub pinned: bool,
    /// When the memory entered `Faded` status (archive-decay anchor).
    pub faded_at: Option<DateTime<Utc>>,
}

/// Payload for creating a new memory item — omits store-managed fields
/// (`id`, `access_count`, `last_accessed_at`, `updated_at`, `faded_at`).
pub struct NewMemoryItem {
    pub scope: MemoryScope,
    pub character_id: String,
    pub user_id: String,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub source: MemorySource,
    pub source_ref: Option<String>,
    pub confidence: MemoryConfidence,
    pub salience: MemorySalience,
    pub affect: AffectAnnotation,
    pub relationship_impact: f32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub status: MemoryStatus,
    pub supersedes_id: Option<i64>,
    pub pinned: bool,
    /// Optional explicit created timestamp; defaults to now on insert.
    pub created_at: Option<DateTime<Utc>>,
}
```

### `Query` / `HybridSearchWeights`

```rust
/// Sole typed-memory search contract (#123). Callers (mind) pre-compute
/// `embedding`; `None` skips vector gather (lexical/recency/commitment only).
pub struct Query<'a> {
    pub query_text: &'a str,
    pub embedding: Option<&'a [f32]>,
    pub character_id: &'a str,
    pub user_id: Option<&'a str>,
    pub model_name: &'a str,
    pub limit: usize,
    pub similarity_threshold: f32,
    pub candidate_pool_size: usize,
    pub query_affect: Option<AffectAnnotation>,
    pub weights: HybridSearchWeights,
    pub decay_half_life_days: f64,
    pub now: DateTime<Utc>,
    pub min_score: f32,
    pub commitment_boost: f32,
    pub recent_fallback_limit: usize,
}

/// Component weights for the hybrid recall score. Store defaults match the
/// historical constants; **product defaults** are supplied by
/// `mind.memory.hybrid_weights` (`MindMemoryConfig`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HybridSearchWeights {
    pub vector: f32,
    pub lexical: f32,
    pub recency: f32,
    pub salience: f32,
    pub confidence: f32,
    pub emotional_match: f32,
    pub relationship: f32,
    pub access_boost: f32,
}
```

`MemorySearchOptions` remains a type alias of `Query` for transitional call sites.
### `ScoredMemory` / `MemoryScoreBreakdown`

```rust
pub struct ScoredMemory {
    pub item: MemoryItem,
    pub breakdown: MemoryScoreBreakdown,
    /// Which retrieval paths surfaced this candidate.
    pub sources: Vec<MemoryCandidateSource>,
}

pub struct MemoryScoreBreakdown {
    pub vector_similarity: f32,
    pub lexical_score: f32,
    pub recency_score: f32,
    pub salience: f32,
    pub confidence: f32,
    pub emotional_match: f32,
    pub relationship: f32,
    pub access_boost: f32,
    pub contradiction_penalty: f32,
    pub stale_penalty: f32,
    // ... plus `total`, the weighted sum used for ranking.
}

pub enum MemoryCandidateSource { Vector, Lexical, Recent, Commitment }
```

### `MemoryStore` typed-memory methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_typed_memory` | `async fn insert_typed_memory(&self, item: &NewMemoryItem) -> Result<i64, MemoryError>` | Inserts a new row, returns its ID. |
| `get_typed_memory` | `async fn get_typed_memory(&self, id: i64) -> Result<Option<MemoryItem>, MemoryError>` | Fetch by primary key. |
| `get_typed_memories_by_character` | `async fn get_typed_memories_by_character(&self, character_id: &str, kind: Option<MemoryKind>, limit: usize, offset: usize) -> Result<Vec<MemoryItem>, MemoryError>` | Paginated listing, optionally filtered by kind. |
| `count_typed_memories` | `async fn count_typed_memories(&self, character_id: &str, kind: Option<MemoryKind>) -> Result<i64, MemoryError>` | Row count for a character (and optional kind). |
| `list_typed_memories_by_source_prefix` | `async fn list_typed_memories_by_source_prefix(&self, character_id: &str, prefix: &str, limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | Used by CCv3 card sync to find previously-indexed rows (e.g. `"ccv3:lorebook:"`). |
| `typed_memory_exists_by_source_ref` | `async fn typed_memory_exists_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<bool, MemoryError>` | Existence check for idempotent re-sync. |
| `get_active_typed_memory_by_source_ref` | `async fn get_active_typed_memory_by_source_ref(&self, character_id: &str, source_ref: &str) -> Result<Option<MemoryItem>, MemoryError>` | Fetch the active row for a given `source_ref`. |
| `archive_typed_memories_by_source_prefixes` | `async fn archive_typed_memories_by_source_prefixes(&self, character_id: &str, prefixes: &[&str], keep_refs: &HashSet<String>) -> Result<usize, MemoryError>` | Archives rows under the given prefixes that are no longer present on re-sync (e.g. removed lorebook entries). |
| `search` | `async fn search(&self, query: &Query<'_>) -> Result<Vec<ScoredMemory>, MemoryError>` | **Sole** typed-memory search entry (#123) — combines optional vector, lexical, recency, salience, confidence, affect, relationship, access, and commitment signals. Callers must pre-compute embeddings. See [`docs/memory/memory.md`](../memory/memory.md#hybrid-memory-search-73). |
| `list_recallable_typed_memories` | `async fn list_recallable_typed_memories(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | `Active`/`Faded`/`Disputed` rows for a character (and optional user scope). |
| `supersede_typed_memory` | `async fn supersede_typed_memory(&self, new_item: &NewMemoryItem, superseded_id: i64) -> Result<i64, MemoryError>` | Atomically inserts the replacement row and marks the prior row `Superseded`. |
| `update_typed_memory_status` | `async fn update_typed_memory_status(&self, id: i64, new_status: MemoryStatus) -> Result<bool, MemoryError>` | Low-level status write; internally delegates to `transition_typed_memory_status`. |
| `transition_typed_memory_status` | `async fn transition_typed_memory_status(&self, id: i64, new_status: MemoryStatus) -> Result<bool, MemoryError>` | Validated lifecycle transition — rejects disallowed transitions (see `forgetting::validate_transition`). |
| `bump_typed_memory_access` | `async fn bump_typed_memory_access(&self, id: i64) -> Result<bool, MemoryError>` | Increments `access_count` and refreshes `last_accessed_at`. Called by recall on surfaced memories. |
| `pin_typed_memory` | `async fn pin_typed_memory(&self, id: i64, pinned: bool) -> Result<bool, MemoryError>` | Pins/unpins a memory (pinned memories skip natural decay). |
| `list_memories_for_decay` | `async fn list_memories_for_decay(&self, character_id: &str, user_id: Option<&str>, statuses: &[MemoryStatus], limit: usize) -> Result<Vec<MemoryItem>, MemoryError>` | Candidates for a natural-decay pass. |
| `apply_natural_decay_batch` | `async fn apply_natural_decay_batch(&self, character_id: &str, user_id: Option<&str>, now: DateTime<Utc>, half_life_days: f64, limit: usize) -> Result<NaturalDecayReport, MemoryError>` | Runs `forgetting::decay_score` + `target_status_after_decay` over a batch and applies transitions. |
| `upsert_memory_embedding` | `async fn upsert_memory_embedding(&self, memory_item_id: i64, model_name: &str, field: &str, embedding: &[f32]) -> Result<(), MemoryError>` | Writes a vector into `memory_embeddings` for a typed memory row. |

```rust
/// Result of a natural-decay batch run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NaturalDecayReport {
    pub faded_count: usize,
    pub archived_count: usize,
}
```

---

## Affect

Persistent per-character (optionally per-user) PAD emotional state, updated every turn by the cognitive runtime and stored so it survives restarts.

### `AffectState`

```rust
pub struct AffectState {
    pub character_id: String,
    pub user_id: String,
    /// Pleasure–displeasure (-1.0..=1.0).
    pub valence: f32,
    /// Excitement–calm (-1.0..=1.0).
    pub arousal: f32,
    /// Control–submission (-1.0..=1.0).
    pub dominance: f32,
    /// Trust toward the user (-1.0..=1.0).
    pub trust: f32,
    /// Affinity / liking toward the user (-1.0..=1.0).
    pub affinity: f32,
    /// Irritation / annoyance level (0.0..=1.0).
    pub irritation: f32,
    /// Curiosity / interest level (0.0..=1.0).
    pub curiosity: f32,
    /// Fatigue / energy depletion (0.0..=1.0).
    pub fatigue: f32,
    /// Human-readable mood label (e.g. `"cheerful"`, `"anxious"`).
    pub mood_label: String,
    /// Natural-language description of the last expression/behaviour.
    pub last_expression: String,
    pub discrete_emotions: Vec<DiscreteEmotion>,
    pub updated_at: Option<DateTime<Utc>>,
}

impl AffectState {
    pub fn neutral(character_id: impl Into<String>) -> Self;
    /// Clamps every numeric field into its valid range.
    pub fn clamp(&mut self);
}
```

### `DiscreteEmotion`

```rust
pub struct DiscreteEmotion {
    /// e.g. `"joy"`, `"sadness"`, `"anger"`, `"fear"`, `"surprise"`, `"neutral"`.
    pub label: String,
    /// Intensity, `0.0..=1.0`.
    pub intensity: f32,
}

impl DiscreteEmotion {
    pub fn new(label: impl Into<String>, intensity: f32) -> Self;
}
```

### Store methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `get_affect_state` | `async fn get_affect_state(&self, character_id: &str) -> Result<AffectState, MemoryError>` | Returns `AffectState::neutral(character_id)` if no row exists yet. |
| `upsert_affect_state` | `async fn upsert_affect_state(&self, state: &AffectState) -> Result<(), MemoryError>` | Clamps the state, then upserts on the `character_id` primary key. |

---

## Commitments

A ledger of companion promises and follow-ups (e.g. *"next time let's talk about X"*), independent of vector recall so they can always be surfaced in prompts.

### `CommitmentStatus`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitmentStatus {
    /// Open and should be surfaced in prompts / recall.
    Active,
    /// User or companion marked the commitment fulfilled.
    Done,
    /// Explicitly cancelled or withdrawn.
    Cancelled,
    /// No longer actionable (expired or superseded without completion).
    Stale,
}
```

### `Commitment` / `NewCommitment`

```rust
pub struct Commitment {
    pub id: Option<i64>,
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    /// Raw due hint from extraction (e.g. `"tomorrow"`, `"次回"`).
    pub due_label: Option<String>,
    /// Linked typed memory row (`MemoryKind::Commitment`).
    pub source_memory_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Payload for creating a new commitment row.
pub struct NewCommitment {
    pub character_id: String,
    pub user_id: String,
    pub title: String,
    pub description: String,
    pub status: CommitmentStatus,
    pub due_at: Option<DateTime<Utc>>,
    pub due_label: Option<String>,
    pub source_memory_id: Option<i64>,
}

/// Lightweight DTO for the Active Commitments prompt section.
pub struct ActiveCommitmentPrompt {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub due_label: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
}
```

### Store methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `insert_commitment` | `async fn insert_commitment(&self, item: &NewCommitment) -> Result<i64, MemoryError>` | Inserts a new commitment row. |
| `get_commitment` | `async fn get_commitment(&self, id: i64) -> Result<Option<Commitment>, MemoryError>` | Fetch by primary key. |
| `get_commitment_by_source_memory` | `async fn get_commitment_by_source_memory(&self, source_memory_id: i64) -> Result<Option<Commitment>, MemoryError>` | Look up the ledger row linked to a typed memory. |
| `list_active_commitments` | `async fn list_active_commitments(&self, character_id: &str, user_id: Option<&str>, limit: usize) -> Result<Vec<Commitment>, MemoryError>` | `Active` rows for prompt injection — no vector search involved. |
| `update_commitment_status` | `async fn update_commitment_status(&self, id: i64, new_status: CommitmentStatus) -> Result<bool, MemoryError>` | Generic status write. |
| `complete_commitment` | `async fn complete_commitment(&self, id: i64) -> Result<bool, MemoryError>` | Marks `Done`. |
| `cancel_commitment` | `async fn cancel_commitment(&self, id: i64) -> Result<bool, MemoryError>` | Marks `Cancelled`. |
| `mark_stale_commitments` | `async fn mark_stale_commitments(&self, now: DateTime<Utc>) -> Result<usize, MemoryError>` | Marks overdue `Active` rows (with an explicit `due_at` in the past) as `Stale`. |

---

## Forgetting Lifecycle

Typed memories age through explicit status transitions instead of hard delete. All logic in `forgetting.rs` is pure/synchronous — the `MemoryStore` methods above call into it.

```rust
pub const FADE_THRESHOLD: f32 = 0.40;
pub const ARCHIVE_THRESHOLD: f32 = 0.15;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid memory status transition: {from:?} -> {to:?}")]
pub struct InvalidTransition {
    pub from: MemoryStatus,
    pub to: MemoryStatus,
}
```

| Function | Signature | Description |
|----------|-----------|-------------|
| `validate_transition` | `fn validate_transition(from: MemoryStatus, to: MemoryStatus) -> Result<(), InvalidTransition>` | Allows `Active → Faded / Superseded / UserDeleted / Disputed` and `Faded → Archived / Disputed`. Everything else is rejected. |
| `emotional_impact` | `fn emotional_impact(affect: AffectAnnotation) -> f32` | Euclidean magnitude of `(valence, arousal)`, normalized to `[0, 1]`. |
| `active_decay_anchor` | `fn active_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` | `last_accessed_at`, falling back to `updated_at`. Used for `Active → Faded` timing. |
| `faded_decay_anchor` | `fn faded_decay_anchor(item: &MemoryItem) -> DateTime<Utc>` | `faded_at`, falling back to `created_at`. Used for `Faded → Archived` timing. |
| `decay_score` | `fn decay_score(item: &MemoryItem, now: DateTime<Utc>, half_life_days: f64) -> f32` | Pinned memories return `1.0`. Otherwise: exponential age decay (`exp(-ln2 * age_days / half_life)`) scaled by salience, confidence, and emotional impact, clamped to `[0, 1]`. |
| `target_status_after_decay` | `fn target_status_after_decay(current: MemoryStatus, score: f32) -> Option<MemoryStatus>` | `Active` + score below `FADE_THRESHOLD` → `Some(Faded)`; `Faded` + score below `ARCHIVE_THRESHOLD` → `Some(Archived)`; otherwise `None`. |

See [`docs/memory/memory.md`](../memory/memory.md#memory-forgetting-lifecycle-76) for the exact decay formula and default thresholds used in production.

---

## Legacy Migration

One-shot conversion from the legacy tables into typed memory, used when a character card adopts the cognitive runtime.

```rust
pub struct LegacyMigrationOptions {
    pub card_name: String,
    pub user_id: String,
    pub embedding_model: String,
    /// When true, reports counts without writing anything.
    pub dry_run: bool,
}

pub struct LegacyMigrationReport {
    pub summaries_migrated: usize,
    pub keyfacts_migrated: usize,
    pub spans_migrated: usize,
    /// Rows skipped because a matching `source_ref` already exists.
    pub skipped_existing: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LegacyRowCounts {
    pub summaries: i64,
    pub keyfacts: i64,
    pub logs: i64,
}

impl LegacyRowCounts {
    pub fn has_legacy_data(self) -> bool;
    pub fn requires_migration_gate(self) -> bool;
}

pub struct MigrationStatus {
    pub card_name: String,
    pub migrated_at: DateTime<Utc>,
    pub legacy_summaries_count: i32,
    pub legacy_keyfacts_count: i32,
    pub legacy_logs_count: i32,
    /// e.g. `"one_shot"`.
    pub strategy: String,
}
```

| Item | Signature | Description |
|------|-----------|-------------|
| `execute_legacy_migration` | `async fn execute_legacy_migration(store: &MemoryStore, options: &LegacyMigrationOptions) -> Result<LegacyMigrationReport, MemoryError>` | Free function that does the actual conversion work; called by `MemoryStore::migrate_legacy`. |
| `MemoryStore::migrate_legacy` | `async fn migrate_legacy(&self, options: &LegacyMigrationOptions) -> Result<LegacyMigrationReport, MemoryError>` | Thin wrapper: `crate::legacy_migration::execute_legacy_migration(self, options).await`. |
| `keyfact_kind_for_key` | `fn keyfact_kind_for_key(key: &str) -> MemoryKind` | Maps `pref_*` / `like` / `dislike` keys to `Preference`; everything else to `UserProfile`. |
| `logs_to_spans` | `fn logs_to_spans(rows: &[LegacyLogRow]) -> Vec<NewMemorySpan>` | Groups conversation-log rows into one span per user/assistant pair. |
| `MemoryStore::count_legacy_rows` | `async fn count_legacy_rows(&self, card_name: &str) -> Result<LegacyRowCounts, MemoryError>` | Row counts across the three legacy tables. |
| `MemoryStore::get_migration_status` | `async fn get_migration_status(&self, card_name: &str) -> Result<Option<MigrationStatus>, MemoryError>` | Reads `memory_migration_meta`. |
| `MemoryStore::is_legacy_migrated` | `async fn is_legacy_migrated(&self, card_name: &str) -> Result<bool, MemoryError>` | Shorthand for `get_migration_status(..).is_some()`. |
| `MemoryStore::mark_migration_complete` | `async fn mark_migration_complete(&self, card_name: &str, counts: LegacyRowCounts, strategy: &str) -> Result<(), MemoryError>` | Records completion in `memory_migration_meta`. |
| `MemoryStore::reset_legacy_memory` | `async fn reset_legacy_memory(&self, card_name: &str) -> Result<(), MemoryError>` | **Destructive.** Truncates legacy tables and clears typed memory for the card. |
| `MemoryStore::ensure_legacy_migration_allowed` | `async fn ensure_legacy_migration_allowed(&self, card_name: &str, require_migration: bool) -> Result<(), MemoryError>` | Returns `MemoryError::LegacyMemoryNotMigrated` when `require_migration` is set and unmigrated legacy rows exist. |

**Migration mapping:**

| Legacy table | Target | Rule |
|---|---|---|
| `conversation_summaries` | `typed_memories` (`Episodic`) | `source_ref = "legacy:summary:{id}"`, embedding copied to `memory_embeddings` |
| `conversation_keyfacts` | `typed_memories` (`Preference` or `UserProfile`) | `source_ref = "legacy:keyfact:{id}"`, kind chosen via `keyfact_kind_for_key` |
| `conversation_logs` | `memory_spans` | One span per user/assistant pair, via `logs_to_spans` |

`Migrator` (in `pub mod migrator`) implements `sea_orm_migration::MigratorTrait` with 7 ordered migrations covering the initial schema, typed memory, affect fields, commitments, the `pinned` flag, `faded_at`, and migration metadata.

---

## Memory Spans & Scene Summaries

Rolling compression over raw conversation logs — one **span** per user/assistant exchange (or a run of them), optionally rolled up into higher compression levels (scene → chapter → arc). The store persists summaries produced by `ene-mind`; it does not call an LLM.

```rust
pub struct NewMemorySpan {
    pub session_id: String,
    pub turn_start: i32,
    pub turn_end: i32,
    pub raw_excerpt: Option<String>,
    pub compressed_summary: Option<String>,
    /// 0 = scene, 1 = chapter, 2 = arc.
    pub compression_level: i32,
}

/// Active scene summary row for prompt injection.
pub struct ActiveSceneSummaryRow {
    pub span_id: i64,
    pub summary: String,
    pub compression_level: i32,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `list_session_ids_for_card` | `async fn list_session_ids_for_card(&self, card_name: &str) -> Result<Vec<String>, MemoryError>` | All session IDs with logs for a character. |
| `memory_span_exists` | `async fn memory_span_exists(&self, session_id: &str, turn_start: i32) -> Result<bool, MemoryError>` | Idempotency check before inserting a span. |
| `insert_memory_span` | `async fn insert_memory_span(&self, span: &NewMemorySpan) -> Result<i64, MemoryError>` | Inserts a new span row. |
| `list_memory_spans_by_session` | `async fn list_memory_spans_by_session(&self, session_id: &str) -> Result<Vec<NewMemorySpan>, MemoryError>` | All spans for a session. |
| `list_memory_spans_by_session_and_level` | `async fn list_memory_spans_by_session_and_level(&self, session_id: &str, compression_level: i32) -> Result<Vec<NewMemorySpan>, MemoryError>` | Filtered by compression level. |
| `get_active_scene_summary` | `async fn get_active_scene_summary(&self, session_id: &str) -> Result<Option<ActiveSceneSummaryRow>, MemoryError>` | Fetches the summary injected into the prompt's **Current Scene** section. |
| `update_span_summary` | `async fn update_span_summary(&self, span_id: i64, summary: &str) -> Result<(), MemoryError>` | Writes the mind-generated `compressed_summary` for a span, once compression has run. |

---

LLM-based conversation summarization lives in `ene-mind::summarizer`; `ene-store` only persists and recalls the resulting summaries. Prompt formatting of recalled summaries lives in `ene-runtime::message_builder` (not in the store — #122).

---

## Lexical Similarity

### `search` — lexical similarity

```rust
pub fn document_lexical_similarity(
    title_a: &str,
    content_a: &str,
    title_b: &str,
    content_b: &str,
) -> f32
```

Jaccard similarity over tokenized `title + content` pairs. Used both inside hybrid typed-memory scoring and for candidate de-duplication in downstream MMR diversification (see [`docs/memory/memory.md`](../memory/memory.md#mmr-diversification-78)).

---

## Configuration: `StoreConfig`

```rust
pub struct StoreConfig {
    pub enabled: bool = false,
    pub db_path: String,
}

impl StoreConfig {
    pub fn resolve_memory_db_path(&self, character_name: &str) -> std::path::PathBuf;
}
```

Loaded via `ene_config::define_config!` under the `store` settings section (see [`ene-config`](./ene-config.md)). Recall and decay policy belongs to `MindMemoryConfig`.

---

## Errors: `MemoryError`

`MemoryError` is a type alias for `EneMemoryError`:

```rust
pub enum EneMemoryError {
    MissingBaseUrl { env_var: String },
    MemoryStoreError(#[from] sea_orm::DbErr),
    MemoryStoreConnectionError(String),
    PromptBuildError(String),
    ApiRequestError(String),
    Config(String),
    Embedding(String),
    /// Wrong length vs. the store's `embedding_dim`, contains NaN/Infinity,
    /// or is otherwise unusable for cosine similarity.
    InvalidEmbedding(String),
    /// A memory lifecycle transition was not permitted (see `forgetting::validate_transition`).
    InvalidTransition { from: MemoryStatus, to: MemoryStatus },
    /// A write to a legacy table was attempted while `LegacyWriteMode::ReadOnly` is set.
    LegacyWriteForbidden,
    /// Legacy rows exist but migration has not completed, and `require_migration` is set.
    LegacyMemoryNotMigrated { card_name: String },
    /// Migration was attempted on a card that already completed migration.
    LegacyAlreadyMigrated { card_name: String },
    Other(String),
}

pub type MemoryError = EneMemoryError;
```

---

## Usage Example

```rust,no_run
use chrono::Utc;
use ene_store::{KeyFact, MemoryStore, RecalledSummary};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open an in-memory store with 4-dimensional embeddings.
    let store = MemoryStore::open_in_memory(4).await?;
    let card_name = "Alicia";

    // Insert a conversation summary with key facts.
    let embedding = vec![1.0_f32, 0.0, 0.0, 0.0];
    let key_facts = vec![
        KeyFact { key: "user_name".into(), value: "Alice".into() },
        KeyFact { key: "favorite_color".into(), value: "blue".into() },
    ];

    let summary_id = store
        .insert_summary(
            "session-001",
            card_name,
            "User Alice said she loves blue and works as a designer.",
            &key_facts,
            &embedding,
            Utc::now(),
        )
        .await?;
    println!("Inserted summary with ID: {summary_id}");

    // Search for related summaries.
    let query_emb = vec![0.9_f32, 0.1, 0.0, 0.0];
    let results: Vec<RecalledSummary> =
        store.search_summaries(&query_emb, card_name, 5, 0.5).await?;

    for (i, rs) in results.iter().enumerate() {
        println!("  {}. [score: {:.3}] {}", i + 1, rs.similarity, rs.entry.summary);
    }

    // Retrieve, update, and delete key facts.
    let facts = store.get_all_keyfacts(card_name).await?;
    println!("Key facts: {facts:?}");

    store.upsert_keyfact(card_name, "favorite_color", "green").await?;
    store.delete_keyfact(card_name, "favorite_color").await?;

    Ok(())
}
```

---

## See Also

- [Cognitive Runtime](../architecture/cognitive-runtime.md) — Memory Arbiter, recall planning, and reranking that build on typed memory
- `ene-mind` — Owns the Memory Arbiter, `RecallPlanner`, and post-turn memory writer that call into this crate
- [`ene-runtime`](./ene-runtime.md) — `MemoryQueryHandle` for external access and actor-level wiring
- [`ene-mind`](./ene-mind.md) — Drives session splits that create legacy summaries via `execute_split`
- [`ene-ai`](./ene-ai.md) — Provides embeddings for storage and search
- [Memory System](../memory/memory.md) — Full design doc: hybrid scoring, MMR diversification, migration, commitment ledger
- [API v2](../architecture/api-v2.md) — Store ownership (`store.*` only; policy under `mind.*`)

