# `ContextManager` / Session Compression & Token Budget Spec

This document details Ene's context management system, including token budget calculation (Context Budget), priority-based prompt ordering (Prompt Packing), and sliding-window session compression.

---

## 1. Token Estimation Heuristics (`tokens.rs`)

To avoid the performance overhead of running full tokenizers on every turn, Ene uses a deterministic character-based estimator.

#### `estimate_tokens`
*   **Signature**: `pub fn estimate_tokens(text: &str) -> usize`
*   **Description**: Estimates LLM tokens contained in a string. Maps CJK (Japanese/Chinese/Korean) character codes to 1-2 tokens and standard English/ASCII character sequences to a 4:1 character-to-token ratio.

#### `tokens_to_chars`
*   **Signature**: `pub const fn tokens_to_chars(tokens: usize) -> usize`
*   **Description**: Maps token budgets back to string character counts for allocations.

#### `truncate_to_tokens`
*   **Signature**: `pub fn truncate_to_tokens(text: &str, max_tokens: usize) -> String`
*   **Description**: Truncates text to fit within a token limit without breaking UTF-8 character boundaries.

---

## 2. Priority-Based Prompt Packing (`budget.rs`)

The `pack_prompt` function assembles target sections within the configured `ContextBudget`.

#### `from_config`
*   **Signature**: `pub const fn from_config(config: &ContextConfig) -> Self`
*   **Description**: Builds budgets using defaults.

#### `from_config_and_hints`
*   **Signature**: `pub const fn from_config_and_hints(config: &ContextConfig, hints: &RecallRecallBudgetHints) -> Self`
*   **Description**: Merges RAG overrides and budget indicators into turn thresholds.

#### `budget_for`
*   **Signature**: `const fn budget_for(&self, kind: PromptSectionKind) -> usize`
*   **Description**: Returns maximum allowed tokens for a section.

#### `validate_context_config`
*   **Signature**: `pub fn validate_context_config(config: &ContextConfig) -> Result<(), CognitionError>`
*   **Description**: Checks if sub-section limits fit within the context capacity.

#### `sort_memories_for_drop`
*   **Signature**: `fn sort_memories_for_drop(memories: &mut [RecalledMemory])`
*   **Description**: Sorts memories so that low-confidence, non-pinned items are dropped first during budget constraints.

#### `memory_section_body`
*   **Signature**: `fn memory_section_body(memories: &[RecalledMemory]) -> String`
*   **Description**: Formats a list of memories into a single markdown string.

#### `set_section_body`
*   **Signature**: `fn set_section_body(sections: &mut [PromptSection], kind: PromptSectionKind, body: String)`
*   **Description**: Updates a section's text content.

#### `estimate_history_tokens`
*   **Signature**: `fn estimate_history_tokens(history: &[HistoryEntry]) -> usize`
*   **Description**: Computes the token count of a message history slice.

#### `trim_history_to_budget`
*   **Signature**: `fn trim_history_to_budget(history: &mut Vec<HistoryEntry>, max_tokens: usize) -> usize`
*   **Description**: Prunes oldest history messages until they fit within limits.

#### `build_sections`
*   **Signature**: `fn build_sections(input: &PackInput, budget: &ContextBudget) -> Vec<PromptSection>`
*   **Description**: Constructs raw sections for prompt packers.

#### `apply_section_budget`
*   **Signature**: `fn apply_section_budget(section: &mut PromptSection)`
*   **Description**: Truncates section text if it exceeds limits.

#### `section_token_total`
*   **Signature**: `fn section_token_total(sections: &[PromptSection]) -> usize`
*   **Description**: Sums token counts across all active prompt sections.

#### `pack_prompt`
*   **Signature**: `pub fn pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt`
*   **Process**:
    1.  Compiles sections and estimates their size.
    2.  If the total size exceeds the budget, it drops or truncates sections in reverse priority order:
        -   `StyleExamples` (dropped first)
        -   `RecalledMemories` (dropped/truncated by confidence)
        -   `ActiveCommitments`
        -   `SceneSummary` / `EmotionSummary`
        -   `History` (pruned oldest first)
        -   `BehaviorContract`
    3.  Asserts that critical sections (e.g. Identity Kernel, User Input) fit.
    4.  Returns the packed prompt and budget metadata.

#### `classify_recalled_memories_owned`
*   **Signature**: `fn classify_recalled_memories_owned(recalled: &[RecalledMemory]) -> (Vec<RecalledMemory>, Vec<RecalledMemory>, Vec<RecalledMemory>)`
*   **Description**: Groups recalled memories by their category.

---

## 3. Sliding-Window Session Compression (`compression.rs`)

To prevent memory loss when conversation history exceeds `trigger_token_limit`, Ene compresses old messages into a "Scene Summary" and prunes them from the active sliding window.

#### `as_i32` / `from_i32`
*   **Signature**: `pub const fn as_i32(self) -> i32` (and `from_i32(value: i32) -> Option<Self>`)
*   **Description**: Serializes/deserializes compression level indicators.

#### `compression_has_usable_summary`
*   **Signature**: `pub fn compression_has_usable_summary(result: &CompressionResult) -> bool`
*   **Description**: Checks if a compression result contains a valid summary.

#### `execute_compression`
*   **Signature**: `pub async fn execute_compression(store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput) -> Result<CompressionResult, CognitionError>`
*   **Description**: Executes the summarization pipeline on conversation history.

#### `spawn_compression_task`
*   **Signature**: `pub fn spawn_compression_task(pending: &mut Option<PendingCompressionTask>, store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput)`
*   **Description**: Spawns a background task for summarization.

#### `poll_compression_result`
*   **Signature**: `pub fn poll_compression_result(pending: &mut Option<PendingCompressionTask>) -> Option<Result<CompressionResult, CognitionError>>`
*   **Description**: Polls the join handle of a running compression task.

#### `evaluate_compression_trigger`
*   **Signature**: `pub fn evaluate_compression_trigger(config: &ContextConfig, turn_count: usize, history_len: usize) -> Option<CompressionReason>`
*   **Description**: Checks if turn or token counts exceed thresholds, returning trigger reasons.

#### `load_active_scene_summary`
*   **Signature**: `pub async fn load_active_scene_summary(store: &MemoryStore, session_id: &str) -> Result<Option<ActiveSceneSummary>, CognitionError>`
*   **Description**: Fetches the most recent scene summary for a session.

#### `run_compression`
*   **Signature**: `async fn run_compression(store: Arc<MemoryStore>, provider: Arc<dyn LlmProvider>, input: CompressionTaskInput) -> Result<CompressionResult, CognitionError>`
*   **Description**: Core compression worker. Summarizes messages, saves the summary to the database, and prunes the session history.

#### `render_turn_excerpt`
*   **Signature**: `fn render_turn_excerpt(turns: &[HistoryEntry]) -> String`
*   **Description**: Formats history messages into a transcript for summarization.

#### `summarize_span`
*   **Signature**: `async fn summarize_span(provider: &dyn LlmProvider, character_name: &str, user_name: &str, excerpt: &str, level: CompressionLevel, timeout_secs: u64) -> Option<String>`
*   **Description**: Prompts the LLM to generate a summary of a transcript excerpt.

#### `maybe_roll_up_chapter`
*   **Signature**: `pub async fn maybe_roll_up_chapter(store: &MemoryStore, provider: Arc<dyn LlmProvider>, session_id: &str, character_name: &str, user_name: &str, config: &ContextConfig) -> Result<Option<CompressionResult>, CognitionError>`
*   **Description**: Consolidates multiple scene summaries into a chapter summary once the count exceeds thresholds.

---

## 4. Facade & Module Methods (`mod.rs`)

#### `validate_config`
*   **Signature**: `pub fn validate_config(config: &ContextConfig) -> Result<(), CognitionError>`
*   **Description**: Validates context configs.

#### `evaluate_compression_trigger` (Facade)
*   **Signature**: `pub fn evaluate_compression_trigger(config: &ContextConfig, turn_count: usize, history_len: usize) -> Option<CompressionReason>`
*   **Description**: Facade method dispatching to inner compression evaluation.

#### `load_scene_summary`
*   **Signature**: `pub async fn load_scene_summary(ctx: TurnContext<'_>) -> Result<Option<ActiveSceneSummary>, CognitionError>`
*   **Description**: Queries the store for active summaries.
