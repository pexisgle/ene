# `RecallPlanner` & Hybrid Memory Recall Specifications

This document defines the technical specifications of Ene's long-term memory recall system. It processes user input and active emotional states, formats hybrid database queries, and filters retrieved factual records.

---

## 1. Data Structures & Enums

### `RecallPlan` (Public / Struct)
The non-binary plan detailing candidate memory queries:
*   `current_topic: String`: The inferred active conversational topic.
*   `semantic_queries: Vec<String>`: Generated search queries for semantic memory facts.
*   `episodic_queries: Vec<String>`: Generated search queries for episodic memories.
*   `required_kinds: Vec<MemoryKind>`: Filter mapping allowed types (`Semantic` or `Episodic`).
*   `scope: RecallScopeFilter`: Filters search scope to specific character and user IDs.
*   `budget: RecallBudgetHints`: Token budget limits and requested result limit.
*   `search: RecallSearchHints`: Contains primary search string, similarity threshold, recency decay half-life, and emotional query anchors.

### `RecalledMemory` (Public / Struct)
A memory item matched and wrapped with a reason for consolidation:
*   `item: MemoryItem`: The raw persistent database record.
*   `reason: RecallReason`: Explanation indicating why this memory was recalled.

### `RecallReason` (Public / Enum)
Reasons indicating why a memory item was fetched:
*   `SimilarTopic`: Vector similarity match on conversational topic.
*   `KeywordMatch`: String match on trigger keywords.
*   `EmotionalMatch`: Episodic memory resembling current emotional valence.
*   `CommitmentLink`: Memory matched via active commitment task ties.
*   `RecencyFallback`: Recent conversation logs loaded as conversational fallback.
*   `Constant`: Permanent world rules loaded directly from character cards.

---

## 2. Planning Phase (`RecallPlanner`)

The `RecallPlanner` is a deterministic query planner that maps current turn conditions onto search parameters. It does not perform network operations or direct DB queries.

### Core Methods

#### `plan`
*   **Signature**: `pub fn plan(input: &RecallPlannerInput<'_>, options: &RecallPlannerOptions) -> Result<RecallPlan, CognitionError>`
*   **Process**:
    1.  Resolves `current_topic` by parsing current user prompt, context snippets, and recent history.
    2.  Invokes `infer_intents` to class current intents (`RecallIntent`) by scanning keyword lists and emotional states.
    3.  Runs `kinds_for_intents` to identify whether `Semantic` or `Episodic` tables should be searched.
    4.  Extracts semantic and episodic query strings based on active commitments and intents.
    5.  Returns the structured `RecallPlan`.

#### `to_memory_search_options`
*   **Signature**:
    ```rust
    pub fn to_memory_search_options<'a>(
        plan: &'a RecallPlan,
        query_embedding: &'a [f32],
        model_name: &'a str,
        now: DateTime<Utc>,
        memory: &MindMemoryConfig,
    ) -> Query<'a>
    ```
*   **Description**: Formats the `RecallPlan` into a SQLite-compatible hybrid query `Query`. Injects the search query embedding vector, current timestamp (`now`) for recency decay calculation, and candidate multipliers.

---

## 3. Execution Phase (`execute_hybrid_recall`)

The runner orchestrates database searches, MMR filtering, and lorebook injections.

### `execute_hybrid_recall`
*   **Signature**: `pub async fn execute_hybrid_recall(config: &MindConfig, input: &ExecuteRecallInput<'_>) -> Result<(RecallPlan, Vec<RecalledMemory>), CognitionError>`
*   **Control Flow**:
    1.  Fetches active promises using `CommitmentLedger::list_active`.
    2.  Constructs the `RecallPlan` via `RecallPlanner::plan`.
    3.  Converts the plan into a database query using `RecallPlanner::to_memory_search_options`.
    4.  Runs the search query against the sqlite-vec backend (`MemoryStore::search`), returning a list of `ScoredMemory` candidates.
    5.  **Diversification (MMR)**:
        Applies Maximal Marginal Relevance (`MemoryDiversifyPipeline::diversify`) to drop highly redundant documents, promoting diversity within the final context packet.
    6.  **Mapping**:
        Uses `RecallResultMapper::map` to wrap candidates into `RecalledMemory` models, assigning reasons based on scores and types.
    7.  **Lorebook Injections**:
        Merges lorebook keyword entries and static world definitions (`maybe_merge_lorebook_recall`) into the retrieved list.
    8.  **Access Recency Bump**:
        Bumps the access counter of recalled memories in the database via `bump_typed_memory_access` to slow down their natural forgetting decay.

---

## 4. Prompt Assembly formatting (`prompt_qualifier.rs`)

Recalled items are formatted into a markdown section inside the system prompt:

*   `format_recalled_content(memories: &[RecalledMemory]) -> String`:
    -   Serializes memories into a bulleted list.
    -   Prefixes each entry with its memory type, pinning status, emotional weights, and matching reason (e.g. `[EPISODIC (recalled because: similar_topic)] content`).
