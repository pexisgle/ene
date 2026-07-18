# Long-Term Memory

SQLite + sqlite-vec + Diesel powered episodic memory with vector similarity search and LLM-driven summarization.

## Initialization

The `EneActor` initializes memory during `reconfigure()`:

1. Create embedding provider from `embedding` config
2. If `store.enabled == true`, call `MemoryStore::open()`
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
    field_key TEXT NOT NULL,        -- "" for single-row fields or ex_N for example rows from ToolRagProfile
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

Each tool has multiple embedding rows (one per field: `summary`, `description`, `capability`, `example`, `negative`). The per-field approach enables `search_tools` to aggregate relevance via max-pool across fields. The `field_key` is `""` for single-row fields or `ex_N` for example rows from `ToolRagProfile`. The `model_name` allows re-embedding with different models.

| Method | Description |
|--------|-------------|
| `upsert_tool_embedding_field(name, field, field_key, model, hash, emb)` | UPSERT a single field embedding |
| `list_tool_embedding_fields()` | List all `(name, field, field_key, model, hash, vector)` rows |
| `delete_tool_embeddings(name)` | Remove all field rows for a tool |
| `search_tools(query_emb, limit, threshold)` | Cosine similarity across all fields, max-pool per tool for Tool RAG |

## Tool DB IPC Server

Tools that need persistent storage (e.g. `ene-tool-fs` for undo, `ene-tool-utility` for todos) access the database through a per-tool IPC server rather than linking `ene-store` directly.

### Architecture

```
Core (ene-runtime)                     Tool binary (e.g. ene-tool-fs)
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
- `GgufEmbeddingProvider` — Local GGUF inference via llama-cpp-2 (last-token pooling), serial batch.
- `HybridRerankProvider` — Wraps a primary embedder with optional LLM for HyDE / rerank.

## Summarization

`ene-mind::summarizer::summarize_conversation()` calls the LLM to produce a structured summary:

```rust
pub struct ConversationSummaryResult {
    pub summary: String,
    pub topics: Vec<String>,
    pub key_facts: Vec<KeyFact>,
}
```

The resulting summary and key facts are persisted by `ene-store`; the store itself has no LLM or embedding-provider dependency.

## Prompt Injection Format

`ene-runtime::message_builder` renders recalled summaries for the prompt (store no longer owns prompt formatters — #122):

```
[Past Conversation Summaries — relevant previous conversations]
- (5 minutes ago) Summary: ...
- (2 hours ago) Summary: ...
```

## Typed Memory & Memory Arbiter (Cognitive Runtime)

The cognitive runtime stores long-term facts in `typed_memories` with explicit `MemoryKind` and `MemoryStatus` lifecycle (`active`, `faded`, `archived`, `disputed`, `superseded`, `user_deleted`).

After each turn, the **LLM extractor** (primary) produces `MemoryCandidate` items. Deterministic patterns cover only explicit remember/forget: remember hits are hints when the LLM succeeds (and fall back when it fails/returns empty/is disabled); forget hits always reach the arbiter as a safety net. Soft signals (preferences, schedules, nicknames) are LLM-only. Tool-grounded candidates apply as configured fallbacks when the LLM does not own the turn. The **Memory Arbiter** (`ene-mind::memory_writer::MemoryArbiter`) validates each candidate against existing memories before calling `MemoryStore::insert_typed_memory` or `MemoryStore::supersede_typed_memory`.

Key store APIs:

| Method | Description |
|--------|-------------|
| `insert_typed_memory(item)` | Insert a new typed memory row |
| `supersede_typed_memory(new_item, old_id)` | Atomically insert replacement and mark prior row `superseded` |
| `update_typed_memory_status(id, status)` | Low-level lifecycle status write (no edge validation) |
| `transition_typed_memory_status(id, status)` | Validated lifecycle transition (#76) |
| `pin_typed_memory(id, pinned)` | Pin / unpin a memory (exempt from natural decay) |
| `apply_natural_decay_batch(...)` | Batch `Active → Faded → Archived` from decay score |
| `search_typed_memories(embedding, ...)` | Vector similarity search over active memories |
| `search(options)` | Hybrid recall with explainable score breakdown |
| `list_recallable_typed_memories(character_id, user_id, limit)` | List `active` / `faded` / `disputed` memories for recall |

See [Cognitive Runtime ADR](../architecture/cognitive-runtime.md#memory-arbiter) for decision rules and thresholds.

### Recall Plan Generation (#72)

`ene-mind::recall::RecallPlanner` turns the current turn context into a deterministic `RecallPlan`. The planner does not query SQLite or call an embedding provider; it prepares search intent for later stages.

Inputs:

- current user input
- recent raw turns
- active scene summary
- current `AffectState`
- active commitments (`ActiveCommitmentPrompt`)
- character id and optional user id

Outputs:

- `semantic_queries` for facts, preferences, relationship context, and lore
- `episodic_queries` for past conversations and recent-turn context
- `required_kinds`, always including `Semantic` / `Episodic`, and including `Commitment` whenever active commitments exist
- `RecallScopeFilter` for character/user scoping
- `RecallBudgetHints` from `mind.context`
- `RecallSearchHints` compatible with `Query` (`similarity_threshold`, `min_score`, recency half-life, optional query affect)

`RecallPlanner::to_memory_search_options` is a helper that maps a plan plus a single query embedding into `Query` for `MemoryStore::search`. It uses only `plan.search.primary_query_text` (the first semantic query). `semantic_queries`, `episodic_queries`, and `required_kinds` remain plan hints for downstream recall execution (multi-query expansion and kind filtering). When `plan.use_hyde` is true, `execute_hybrid_recall` generates a hypothetical document via `ene_ai::hyde_document`, embeds it as `EmbeddingKind::Hyde`, and blends it with the query embedding using `mind.memory.hyde_blend` before search.

### Hybrid Memory Search (#73)

Typed memory recall can combine multiple signals instead of vector similarity alone. Use `Query` with `MemoryStore::search` to obtain `ScoredMemory` results that include a `MemoryScoreBreakdown` and the recall sources (`vector`, `lexical`, `recent`, `commitment`).

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

`Query` also supports:

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

### Explainable Recall Reasons (#74)

`MemoryStore::search` returns raw [`ScoredMemory`](../../../crates/ene-store/src/typed_memory.rs) values. Reason assignment lives in `ene-mind::recall`: downstream recall execution converts those results into `RecalledMemory` DTOs. Each result includes:

- `item` — the typed memory row
- `reason` — a single primary `RecallReason` for UX, debug, and prompt introspection
- `score_breakdown` — the same `MemoryScoreBreakdown` from hybrid search
- `sources` — contributing recall sources (`vector`, `lexical`, `recent`, `commitment`)

`RecallReason` variants:

| Reason | Typical signal |
|--------|----------------|
| `similar_topic` | Default for vector/lexical hybrid match |
| `recent_conversation` | `recent` source or `Episodic` kind |
| `active_promise` | `commitment` source or `Commitment` kind |
| `character_lore` | `MemorySource::Ccv3` (CCv3 lorebook) |
| `user_preference` | `Preference` or `UserProfile` kind |
| `emotional_continuity` | `Affective` kind, or `emotional_match >= 0.85` |
| `pinned` | Reserved for future user-pinned memories (not inferred yet) |

Use `RecallResultMapper::map`, `RecallPlanner::explain_results`, `RecalledMemory::from_scored`, or `explain_scored_memories` to map hybrid search output. All types are `Serialize`/`Deserialize` for CLI inspect and JSON snapshots.

Reason priority (first match wins): `ActivePromise` → `CharacterLore` → `UserPreference` → `EmotionalContinuity` → `RecentConversation` → `SimilarTopic`.

### CCv3 Character Memory Index (#82–#84)

The mind runtime's `CognitionEngine::sync_character_memories` compiles CCv3 card data into character-scoped typed memories:

| Source | `source_ref` prefix | `MemoryKind` | Notes |
|--------|----------------------|--------------|-------|
| Lorebook entry | `ccv3:lorebook:{id}` | `Semantic` | Constant entries are `pinned`; trigger keys are stored in **content** as `Triggers: …` |
| `mes_example` chunk | `ccv3:style:{index}` | `Procedure` | Selected per turn for the Style Examples prompt section |

Rows under these prefixes that are no longer present in the card are archived on reindex. Rows with the same `source_ref` but changed content are **superseded** and re-embedded. The session caches the combined lorebook/style hash in `MemoryContext.ccv3_memory_hash` to skip redundant sync work across turns. Store helpers: `list_typed_memories_by_source_prefix`, `get_active_typed_memory_by_source_ref`, `archive_typed_memories_by_source_prefixes`, `supersede_typed_memory`.

### Tool Result Grounding (#92)

Tool execution results are grounded into typed memory through the cognitive post-turn writer:

- Tool outcomes are captured as `ToolResultSummary { tool_name, success, summary }` per call.
- Raw outputs are sanitized and truncated by `max_summary_chars`; screenshot payloads are reduced to a fixed sentinel message.
- Successful calls produce `Procedure` memories (`source_ref` prefix: `tool:`).
- Failed calls produce `Reflection` memories so future behavior can avoid repeating the same failed path.
- Short user-visible successes may also produce `Episodic` memories when enabled.

This keeps memory useful for recall while preventing large raw tool outputs from being persisted as-is.

### Optional Memory Reranking (#77)

After hybrid search, downstream recall execution may optionally rerank the top candidates before mapping to `RecalledMemory`:

1. `MemoryStore::search` returns `ScoredMemory` rows ordered by hybrid `total`.
2. When `mind.memory.rerank_enabled` is `false` (default), order is unchanged.
3. When enabled, `MemoryRerankPipeline` sends up to `rerank_candidate_limit` top candidates to an LLM reranker. The prompt includes only the recall question and each candidate's `content` — no title, source, kind, or user metadata. Candidates beyond the limit keep their original hybrid order and are appended after the reranked head.
4. On timeout, provider error, or malformed structured output, the pipeline falls back to the hybrid search order.
5. `RecallResultMapper::map` converts the (possibly reranked) list into explainable `RecalledMemory` values.

**Order vs scores:** Reranking changes list order only. Each result's `score_breakdown.total` remains the hybrid-search score, so the first recalled item may display a lower `total` than items ranked below it.

**Privacy & cost:** Enabling rerank sends stored memory content to the configured LLM provider on every recall that has multiple candidates. This adds latency and token cost proportional to candidate count and content length. Keep `rerank_candidate_limit` conservative unless a dedicated rerank model is configured. Parse failures log structural error details and response length only — not the full LLM payload.

**Tracing:** Rerank latency and status are logged under `component = "MemoryRerank"` (`elapsed_ms`, `candidate_count`, `reranked_count`, `tail_count`, `outcome`, and `skip_reason` when skipped).

### MMR Diversification (#78)

After hybrid search and before optional LLM reranking, downstream recall execution applies deterministic MMR diversification via `MemoryDiversifyPipeline`:

1. `MemoryStore::search` returns `ScoredMemory` rows ordered by hybrid `total`.
2. **Cluster dedup** merges near-duplicate candidates (lexical Jaccard similarity on title + content ≥ `mmr_duplicate_cluster_threshold`), keeping the highest-scoring representative per cluster.
3. **Greedy MMR** selects up to `RecallPlan.budget.result_limit` items using `λ * relevance - (1-λ) * max_similarity_to_selected`, where `relevance` is `score_breakdown.total` normalized to the pool maximum and pairwise similarity uses the same lexical metric. A small `mmr_source_diversity_bonus` is added when a candidate introduces a recall source type not yet present in the selected set.
4. **Kind quotas** reserve minimum slots for semantic, episodic, user profile, and commitment memories (`mmr_min_slots_*`). Kinds listed in `RecallPlan.required_kinds` (including `preference`, `relationship`, `affective`, and `procedure`) receive at least one slot when budget allows. When the sum of minimums exceeds `result_limit`, slots are allocated by priority: commitment → user profile → preference → semantic → episodic → relationship → affective → procedure → reflection.
5. When `mind.memory.mmr_enabled` is `false`, the pipeline truncates to `result_limit` without reordering.
6. Optional LLM reranking (#77) runs on the diversified list. Hybrid scores on each `ScoredMemory` are never modified.

**Order vs scores:** MMR and reranking change list order only. Each result's `score_breakdown.total` remains the hybrid-search score.

**Tracing:** Diversification is logged under `component = "Recall"`, `stage = "diversify"` (`input_count`, `pool_count`, `output_count`, `clusters_merged`, `kind_distribution`).

### Memory Forgetting Lifecycle (#76)

Typed memories age through explicit status transitions instead of hard delete. Natural decay and user explicit forget are separate paths.

**Allowed single-step transitions** (`ene-store::forgetting::validate_transition`):

| From | To |
|------|-----|
| `active` | `faded`, `superseded`, `user_deleted`, `disputed` |
| `faded` | `archived`, `disputed` |

All lifecycle status writes go through `transition_typed_memory_status` (including `update_typed_memory_status`, which delegates to it). `supersede_typed_memory` continues to handle transactional `active/faded/disputed → superseded` plus successor insert.

**`faded_at` column:** Set when a memory transitions `active → faded`, using the active decay anchor at transition time (`last_accessed_at`, else `updated_at`). Existing `faded` rows are backfilled with `faded_at = updated_at` on migration. Archive decay uses `faded_at` (fallback: `created_at`), not the post-transition `updated_at`.

**Natural decay score** (`decay_score`, distinct from hybrid-recall `recency_score`):

```text
retention =
  exp(-ln2 * age_days / half_life)
  * (0.5 + 0.5 * salience)
  * (0.5 + 0.5 * confidence)
  * (0.7 + 0.3 * emotional_impact)
```

- **Active fade decisions:** `age_days` from `active_decay_anchor` (`last_accessed_at`, else `updated_at`).
- **Faded archive decisions:** `age_days` from `faded_decay_anchor` (`faded_at`, else `created_at`).
- `half_life` comes from `mind.memory.default_forgetting_half_life_days`.
- `pinned` memories return retention `1.0` and are skipped.

**Thresholds (defaults):**

- `retention < 0.40` and `active` → `faded`
- `retention < 0.15` and `faded` → `archived`

**Explicit vs natural forget:**

| Path | Trigger | Result |
|------|---------|--------|
| User forget | Memory Arbiter `MarkUserDeleted` | Immediate `user_deleted` (decay bypassed) |
| Natural decay | `ForgettingLifecycle::apply` when `decay_enabled` is true | `active → faded → archived` |

Post-turn `ForgettingLifecycle::apply` runs from `streaming_cognitive.rs` after each assistant turn when memory is enabled. Recall bumps `last_accessed_at` via `bump_typed_memory_access` for surfaced memories.

**Prompt uncertain markers:** When typed recall maps a `RecalledMemory` into prompt text, `format_recalled_content` prefixes:

- `[uncertain] ` for `faded` memories (and low-confidence `active` memories)
- `[disputed] ` for `disputed` memories

Legacy `conversation_keyfacts` receive these markers only when explicitly converted by the migration tooling; normal mind recall does not merge `recall_context` rows.

## Migration from Legacy Tables

The mind runtime writes new memories to typed memory only. Legacy tables (`conversation_summaries`, `conversation_keyfacts`) are **read-only** until you migrate or reset.

### Mapping rules (one-shot migration)

| Legacy table | Target | Rules |
|--------------|--------|-------|
| `conversation_summaries` | `typed_memories` (`Episodic`) | `content` ← summary text; `confidence = 0.7`, `salience = 0.5`; embedding copied to `memory_embeddings`; `source_ref = legacy:summary:{id}` |
| `conversation_keyfacts` | `UserProfile` or `Preference` | Keys matching `pref_*`, `like`, or `dislike` → `Preference`; otherwise → `UserProfile`; `title` = key, `content` = value; `source_ref = legacy:keyfact:{id}` |
| `conversation_logs` | `memory_spans` | One span per user/assistant pair (or single message when unpaired); `raw_excerpt` holds message text; `compressed_summary` filled by rolling compression (#79) at runtime |

At runtime (cognitive path), `ene-mind::context::compression` writes scene-level spans when turn count or context pressure exceeds `mind.context` thresholds. Active scene summaries are injected into the **Current Scene** section of `PromptPacket` via `MemoryStore::get_active_scene_summary`. Raw `conversation_logs` are always preserved.

Migration progress is recorded in `memory_migration_meta` per character card.

### User options

1. **Do nothing (read-only legacy data)** — Until you migrate, legacy summaries and key facts remain outside normal mind recall. New extracted memories go to typed tables only. Raw conversation logs continue to append to `conversation_logs` on each turn.
2. **`/memory migrate legacy`** — One-shot conversion inside a single transaction; sets the migration marker. After migration, recall uses typed memory only for that card.
3. **`/memory reset legacy --yes`** — Truncates legacy tables and clears typed memory for the card (destructive; requires confirmation). Memory spans are removed only for sessions linked to that card's logs.

### Strict mode

Set `mind.memory.require_migration = true` to block recall when **legacy summaries or keyfacts** exist but migration has not completed. Ongoing `conversation_logs` from normal chat do **not** trigger this gate. The store returns `LegacyMemoryNotMigrated` with reset/migrate guidance.

### Reset guidance

To start fresh without migrating:

```bash
ene-cli
/memory reset legacy --yes
```

Or delete the SQLite file under your user data directory and restart (all memory for all characters is lost).

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

**Runtime wiring:** `CommitmentLedger::arbitrate_apply_and_sync` writes commitment candidates **ledger-first** (sole SoT, #124) and arbitrates other kinds via the Memory Arbiter — there is no typed→ledger dual-write / `sync_from_applied_decisions`. Optional typed rows may reference `typed_memories.commitment_id`. `active_prompt_candidates` produces lightweight DTOs for the Active Commitments `PromptPacket` section (#87). CLI list/complete commands are available via `/commitments` (#94).

## Memory Journal (Desktop UX)

The desktop **Memory Journal** page (`ene-desktop` Settings → Memory) exposes typed memory, affect state, and active commitments for inspection.

### Browse mode (default)

- Lists memories for the current character scoped to the configured user **plus** global rows (`user_id = ""`).
- Shows kind, status, scope, confidence, salience, source metadata, and pin state.
- Lifecycle actions are gated by `MemoryJournalPresenter` to match store rules:
  - **Active:** Pin/Unpin, Forget (`user_forget_typed_memory`), Dispute
  - **Faded:** Pin/Unpin, Archive (`transition_typed_memory_status`), Dispute, Restore (`user_restore_typed_memory`)
  - **Archived / UserDeleted / Superseded / Disputed:** Pin/Unpin, Restore
- Filters: show deleted, archived, and superseded rows independently.

### Recall debug mode

- Optional query box runs `search_typed_memories_explained` (hybrid search + #74 recall reasons).
- Displays `RecallReason` labels and score breakdown (vector, lexical, recency, salience, confidence).

### APIs

| Layer | Method |
|-------|--------|
| `ene-store` | `list_journal_memories`, `user_restore_typed_memory`, `user_forget_typed_memory` |
| `ene-runtime` | `MemoryQueryHandle::list_journal_memories`, `user_restore_typed_memory`, `search_typed_memories_explained` |
