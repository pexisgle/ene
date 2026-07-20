# `RecallPlanner` & Hybrid Memory Recall Specifications

This document defines Ene's long-term memory recall system. It processes user inputs, active topics, intents, and emotional states, formats hybrid database queries, runs vector similarity searches, filters retrieved factual records, and dedupes them via MMR.

---

## 1. Data Structures & Enums

### `RecallPlan` (Public / Struct)
The non-binary plan detailing candidate memory queries:
*   `current_topic: String`: Inferred active conversational topic.
*   `semantic_queries: Vec<String>`: Generated search queries for semantic memory facts.
*   `episodic_queries: Vec<String>`: Generated search queries for episodic memories.
*   `required_kinds: Vec<MemoryKind>`: Filter mapping allowed types (`Semantic` or `Episodic`).
*   `scope: RecallScopeFilter`: Filters search scope to specific character and user IDs.
*   `budget: RecallBudgetHints`: Token budget limits and requested result limit.
*   `search: RecallSearchHints`: Primary search query, similarity thresholds, recency decay factors, and emotional query anchors.

---

## 2. Planning Phase (`RecallPlanner`)

The `RecallPlanner` is a deterministic query planner that maps current turn conditions onto search parameters.

#### `from_config`
*   **Signature**: `pub fn from_config(context: &ContextConfig, memory: &MindMemoryConfig) -> Self`
*   **Description**: Constructs a new `RecallPlanner` instance configured with the active token budgets and memory decay settings.

#### `plan`
*   **Signature**: `pub fn plan(input: &RecallPlannerInput<'_>, options: &RecallPlannerOptions) -> Result<RecallPlan, CognitionError>`
*   **Process**:
    1.  Resolves `current_topic` by parsing current user prompt, context snippets, and recent history.
    2.  Invokes `infer_intents` to classify current intents (`RecallIntent`) by scanning keyword lists and emotional states.
    3.  Runs `kinds_for_intents` to identify whether `Semantic` or `Episodic` tables should be searched.
    4.  Extracts semantic and episodic query strings based on active commitments and intents.
    5.  Returns the structured `RecallPlan`.

#### `to_query`
*   **Signature**: `pub fn to_query<'a>(plan: &'a RecallPlan, embedding: Option<&'a [f32]>, model_name: &'a str, now: DateTime<Utc>, memory: &MindMemoryConfig) -> Query<'a>`
*   **Description**: Maps a `RecallPlan` into a database-ready query model for simple vector/lexical execution.

#### `to_memory_search_options`
*   **Signature**: `pub fn to_memory_search_options<'a>(plan: &'a RecallPlan, query_embedding: &'a [f32], model_name: &'a str, now: DateTime<Utc>, memory: &MindMemoryConfig) -> Query<'a>`
*   **Description**: Formats the `RecallPlan` into a SQLite-compatible hybrid query `Query`. Injects the search query embedding vector, current timestamp (`now`) for recency decay calculation, and candidate multipliers.

#### `explain_results`
*   **Signature**: `pub fn explain_results(scored: Vec<ScoredMemory>) -> Vec<RecalledMemory>`
*   **Description**: Wraps lists of scores with reasons.

#### `semantic_queries`
*   **Signature**: `fn semantic_queries(topic: &str, intents: &[RecallIntent], commitments: &[ActiveCommitmentPrompt]) -> Vec<String>`
*   **Description**: Generates text queries for semantic memory lookups by combining conversational topics, inferred user intents, and active task lists.

#### `episodic_queries`
*   **Signature**: `fn episodic_queries(topic: &str, recent_turns: &[super::input::RecallTurn<'_>], intents: &[RecallIntent]) -> Vec<String>`
*   **Description**: Builds target query strings for episodic conversation segment lookups.

#### `query_affect`
*   **Signature**: `fn query_affect(state: &AffectState) -> Option<AffectAnnotation>`
*   **Description**: Compiles current PAD emotional weights into query coefficients.

#### `clamp_unit_signed`
*   **Signature**: `const fn clamp_unit_signed(value: f32) -> f32`
*   **Description**: Utility function to clamp a float to the `[-1.0, 1.0]` range.

#### `push_query`
*   **Signature**: `fn push_query(queries: &mut Vec<String>, query: &str)`
*   **Description**: Safe push utility that filters duplicate strings and ignores empty queries.

---

## 3. Topic & Intent Classification (`topic.rs` & `intent.rs`)

#### `current_topic`
*   **Signature**: `pub fn current_topic(user_input: &str, recent_turns: &[RecallTurn<'_>], scene_summary: Option<&str>) -> Option<String>`
*   **Description**: Resolves active conversational topics by scanning user prompts, fallback histories, and scene contexts.

#### `normalize_text`
*   **Signature**: `pub fn normalize_text(text: &str) -> Option<String>`
*   **Description**: Strips casing, whitespace, and punctuation for keyword matching.

#### `recent_user_turn`
*   **Signature**: `pub fn recent_user_turn(recent_turns: &[RecallTurn<'_>]) -> Option<String>`
*   **Description**: Extracts the last user message from memory history logs.

#### `contains_case_insensitive`
*   **Signature**: `fn contains_case_insensitive(haystack: &str, needle: &str) -> bool`
*   **Description**: Case-insensitive substring match.

#### `truncate_chars`
*   **Signature**: `fn truncate_chars(text: &str, max_chars: usize) -> String`
*   **Description**: Safely truncates strings at character boundaries.

#### `infer_intents`
*   **Signature**: `pub fn infer_intents(topic: &str, affect: Option<&AffectState>) -> Vec<RecallIntent>`
*   **Description**: Appraises keywords and PAD values to classify user interests (e.g. searching for user profile details, assistant lore, or emotional memories).

#### `kinds_for_intents`
*   **Signature**: `pub fn kinds_for_intents(intents: &[RecallIntent], has_commitments: bool) -> Vec<MemoryKind>`
*   **Description**: Maps abstract intents to database memory table categories.

#### `contains_any`
*   **Signature**: `pub fn contains_any(text: &str, needles: &[&str]) -> bool`
*   **Description**: Verifies if any target keyword resides in a normalized text block.

#### `dedupe_intents`
*   **Signature**: `fn dedupe_intents(intents: &mut Vec<RecallIntent>)`
*   **Description**: In-place deduplication of classified intents.

#### `push_unique`
*   **Signature**: `fn push_unique(kinds: &mut Vec<MemoryKind>, kind: MemoryKind)`
*   **Description**: Pushes unique `MemoryKind` enums into lists.

---

## 4. Execution Phase (`execute_hybrid_recall`)

The runner orchestrates database searches, MMR filtering, and lorebook injections.

#### `execute_hybrid_recall`
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

#### `maybe_merge_lorebook_recall`
*   **Signature**: `async fn maybe_merge_lorebook_recall(config: &MindConfig, input: &ExecuteRecallInput<'_>, recalled: Vec<RecalledMemory>) -> Result<Vec<RecalledMemory>, CognitionError>`
*   **Description**: Combines vector recalled memories with keyword-triggered lorebook entries from character cards.

#### `bump_recalled_memory_access`
*   **Signature**: `async fn bump_recalled_memory_access(store: &MemoryStore, recalled: &[RecalledMemory])`
*   **Description**: Bumps the access counters of recalled memories in the database to slow down their natural forgetting decay.

#### `merge_lorebook_recall`
*   **Signature**: `pub async fn merge_lorebook_recall(store: &MemoryStore, character_id: &str, card: Option<&CharacterCardV3>, user_input: &str, recent_turns: &[RecallTurn<'_>], recalled: Vec<RecalledMemory>) -> Result<Vec<RecalledMemory>, CognitionError>`
*   **Description**: Compiles keyword match dictionaries and scans conversational lines, fetching matching lore records from the store.

#### `recalled_memory_from_item`
*   **Signature**: `fn recalled_memory_from_item(item: MemoryItem) -> RecalledMemory`
*   **Description**: Wraps memory records into recalled structures carrying the `RecallReason::Constant` flag.

#### `lorebook_entry_matches`
*   **Signature**: `fn lorebook_entry_matches(item: &MemoryItem, book: &ene_config::Lorebook, scan_text: &str, regex_cache: &std::collections::HashMap<String, regex::Regex>) -> bool`
*   **Description**: Checks if an item matches lorebook regexes and keys.

---

## 5. Result Selection & Diversification (`diversify.rs`)

Avoids presenting redundant or near-duplicate memory contexts to the LLM.

#### `from_config`
*   **Signature**: `pub const fn from_config(config: &MindMemoryConfig) -> Self`
*   **Description**: Configures quota slots, similarity limits, and multipliers for MMR.

#### `diversify`
*   **Signature**: `pub fn diversify(candidates: Vec<ScoredMemory>, plan: &RecallPlan, options: MemoryDiversifyOptions) -> Vec<ScoredMemory>`
*   **Process**:
    1.  Deduplicates candidates using token clustering thresholds.
    2.  Allocates minimum quota slots to specific kinds of memories (e.g. keeping at least one active promise slot).
    3.  Runs the `greedy_mmr` algorithm to select diverse documents based on relevance and similarity.
    4.  Returns the selected candidates.

#### `truncate`
*   **Signature**: `fn truncate(mut candidates: Vec<ScoredMemory>, limit: usize) -> Vec<ScoredMemory>`
*   **Description**: Slices results to fit limits.

#### `item_similarity`
*   **Signature**: `fn item_similarity(a: &ScoredMemory, b: &ScoredMemory) -> f32`
*   **Description**: Measures document semantic overlaps using cosine similarity on memory embeddings.

#### `cluster_dedup`
*   **Signature**: `fn cluster_dedup(mut candidates: Vec<ScoredMemory>, threshold: f32) -> Vec<ScoredMemory>`
*   **Description**: Groups near-duplicate records and retains only the highest scoring candidate from each group.

#### `greedy_mmr`
*   **Signature**: `fn greedy_mmr(pool: &[ScoredMemory], limit: usize, options: MemoryDiversifyOptions) -> Vec<ScoredMemory>`
*   **Description**: Runs the Maximal Marginal Relevance optimization loop to maximize information coverage.

#### `mmr_score`
*   **Signature**: `fn mmr_score(candidate: &ScoredMemory, selected: &[ScoredMemory], options: MemoryDiversifyOptions, max_relevance: f32) -> f32`
*   **Description**: Computes candidate MMR scores based on relevance and diversity.

#### `source_diversity_bonus`
*   **Signature**: `fn source_diversity_bonus(candidate: &ScoredMemory, selected: &[ScoredMemory], bonus: f32) -> f32`
*   **Description**: Grants score bonuses if candidates belong to underrepresented sources.

#### `effective_min_slots`
*   **Signature**: `fn effective_min_slots(plan: &RecallPlan, options: MemoryDiversifyOptions, limit: usize) -> Vec<(MemoryKind, usize)>`
*   **Description**: Calculates target slots per memory category.

#### `apply_kind_quotas`
*   **Signature**: `fn apply_kind_quotas(selected: &mut Vec<ScoredMemory>, pool: &[ScoredMemory], plan: &RecallPlan, options: MemoryDiversifyOptions, limit: usize)`
*   **Description**: Ensures minimum quota slot allocations are satisfied.

#### `best_pool_candidate_for_kind`
*   **Signature**: `fn best_pool_candidate_for_kind(pool: &[ScoredMemory], selected: &[ScoredMemory], kind: MemoryKind) -> Option<ScoredMemory>`
*   **Description**: Returns the highest-scoring candidate for a specific memory category.

#### `lowest_scoring_swappable_index`
*   **Signature**: `fn lowest_scoring_swappable_index(selected: &[ScoredMemory], mins: &[(MemoryKind, usize)]) -> Option<usize>`
*   **Description**: Identifies low scoring items that can be swapped out to satisfy minimum category quotas.

#### `kind_counts`
*   **Signature**: `fn kind_counts(selected: &[ScoredMemory]) -> std::collections::HashMap<&'static str, usize>`
*   **Description**: Counts items by category in a candidate list.

---

## 6. Prompt Qualification formatting (`prompt_qualifier.rs`)

#### `format_recalled_content`
*   **Signature**: `pub fn format_recalled_content(memory: &RecalledMemory) -> String`
*   **Description**: Serializes a recalled memory item into a markdown bullet point carrying metadata prefixes (e.g. source, category, and matching reason).

#### `recall_content_qualifier`
*   **Signature**: `pub fn recall_content_qualifier(memory: &RecalledMemory) -> Option<&'static str>`
*   **Description**: Prefixes uncertainty markers (such as "Uncertain:") for faded memories or those with low confidence scores.
