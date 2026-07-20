# `ContextManager` / Session Compression & Token Budget Spec

This document details Ene's context management system, including token budget calculation (Context Budget), priority-based prompt ordering (Prompt Packing), and sliding-window session compression.

---

## 1. Token Estimation Heuristics (`tokens.rs`)

To avoid the performance overhead of running full tokenizers on every turn, Ene uses a deterministic character-based estimator.
*   **Estimation Ratios**:
    -   English Text: Approx. 4 characters = 1 token.
    -   CJK Text (Japanese, Chinese, Korean): 1 character = 1 to 2 tokens (determined via CJK unicode character boundary classification).
*   **Core Functions**:
    -   `estimate_tokens(text: &str) -> usize`: Returns estimated token count.
    -   `truncate_to_tokens(text: &str, max_tokens: usize) -> &str`: Slices text to fit within token boundaries.

---

## 2. Priority-Based Prompt Packing (`budget.rs`)

The `pack_prompt` function assembles target sections within the configured `ContextBudget`.

### Section Priorities & Trimming Decisions
When token pressure is detected, sections are truncated or dropped following this priority queue:

| Priority | Section Name | Trimming / Dropping Behavior |
|---|---|---|
| 1 (Highest) | **User Input** / **Platform Constraints** | Protected. Throw an error if they exceed the budget. |
| 2 | **Identity Kernel** | Protected. Holds the mascot's core persona. |
| 3 | **Output/Expression Contract** | Protected. Instructs the format of visual cues. |
| 4 | **Behavior Contract** | Appends creator guidelines. |
| 5 | **Recent History** | Protected down to a minimum turn window. Truncates older history first. |
| 6 | **Emotion/Scene Summary** | Current situational environment indicators. |
| 7 | **Active Commitments** | Active promises/tasks. |
| 8 | **Recalled Memories** | Dropped first (least similar first) under token pressure. |
| 9 (Lowest) | **Style Examples** | Dropped completely first if space is unavailable. |

*   `pack_prompt(input: PackInput, budget: &ContextBudget) -> PackedPrompt`:
    Iteratively deducts token costs from the budget and packs sections. Trimming metadata is compiled in `BudgetMeta`.

---

## 3. Sliding-Window Session Compression (`compression.rs`)

To prevent memory loss when conversation history exceeds `trigger_token_limit`, Ene compresses old messages into a "Scene Summary" and prunes them from the active sliding window.

### 1. Trigger Verification (`evaluate_compression_trigger`)
*   Monitors history message count and total token count. Triggers `CompressionReason` (`TokenLimitExceeded` or `TurnLimitExceeded`) if thresholds are crossed.

### 2. Background Compression Task
1.  **Orchestration**:
    The actor spawns the compression pipeline via `spawn_compression_task` after the terminal event.
2.  **LLM Summarization (`summarize_conversation`)**:
    -   Extracts messages outside the active window (e.g. older than 20 turns).
    -   Queries the summarizer model to compile an `ActiveSceneSummary` capturing key topics, events, and resolutions.
3.  **Persistence & Pruning**:
    -   Writes the summary to the `memory_spans` database table.
    -   Deletes the summarized messages from the in-memory `ConversationSession` history list.
4.  **Prompt Injection**:
    Subsequent turns inject the new `ActiveSceneSummary` into the prompt packet in place of the pruned raw messages.
