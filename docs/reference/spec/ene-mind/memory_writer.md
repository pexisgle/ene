# `MemoryWriter` / Long-Term Facts Consolidation & Decay Spec

This document details the background memory consolidation system, covering deterministic regex triggers, LLM-driven fact extraction, candidate arbitration (Memory Arbiter), and temporal decay forgetting.

---

## 1. Data Structures

### `MemoryWriteProviders<'a>` (Public / Struct)
The external AI providers passed to the background write executor:
*   `llm: Option<&'a dyn LlmProvider>`: LLM provider for structured consolidation prompts.
*   `embedder: Option<&'a dyn EmbeddingProvider>`: Embedding provider for duplicate verification.

### `TurnInput` (Private / Conversational data)
*   `user_message: &str`
*   `assistant_message: &str`
*   `tool_results: &[ToolResultSummary]`: Cues summarizing active tool executions during the turn.

---

## 2. Consolidation Lifecycle (`MemoryWriter`)

### `write_memories`
*   **Signature**:
    ```rust
    pub async fn write_memories(
        store: &MemoryStore,
        config: &MindConfig,
        input: &PostTurnInput<'_>,
        providers: MemoryWriteProviders<'_>,
    ) -> Result<(), CognitionError>
    ```
*   **Control Flow**:
    1.  **Deterministic Extraction**:
        -   Runs `deterministic::extract_with_tool_grounding` to parse target messages for user-directed memorization keywords (e.g. "remember that...") via regex.
        -   Extracts candidates from tool result payloads based on configuration rules.
    2.  **LLM Extraction**:
        -   If `providers.llm` is configured, it executes `llm::extract_with_timeout`. The LLM scans the turn dialogue to extract semantic facts (such as user preferences).
        -   If the LLM output is empty or connection fails, the process falls back to the deterministic candidates as a safety net.
    3.  **Arbitration (Memory Arbiter)**:
        -   Runs vector searches for each candidate to detect semantically duplicate memories in the database.
        -   Resolves conflicts by comparing confidence scores (Superseding, disputing, or ignoring candidates).
    4.  **Persistence**:
        -   Generates embeddings for final approved candidates and writes them to sqlite.

### `apply_forgetting`
*   **Signature**: `pub async fn apply_forgetting(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **Description**: Launches the decay phase. Invokes `ForgettingLifecycle::apply` to transition aged, neglected memory items from `Active` to `Faded` or `Archived` statuses.

### `finalize_turn` (Synchronous Pre-process)
*   **Signature**: `pub async fn finalize_turn(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **Description**: Saves the turn's plain chat logs to `conversation_logs` and marks commitments complete. Runs synchronously before streaming finishes.

---

## 3. Modular Components

### 1. Deterministic Extraction (`deterministic.rs` & `tool_grounding.rs`)
*   **Regex Parsing**: Uses language-specific regular expressions to match user instructions.
*   **Tool Grounding**: Generates memory candidates from tool logs (e.g., noting that a file was successfully written).

### 2. LLM Extraction (`llm.rs`)
*   **Structured Output**: Employs JSON schemas to prompt the model for structured candidates (containing fields like Title, Content, Confidence, and EmotionalImpact).
*   **Extraction Timeout**: Enforces `extraction_timeout_secs`, automatically aborting the request and falling back to deterministic extraction if exceeded.

### 3. Memory Arbiter (`arbiter.rs`)
Validates and deduplicates candidate items against the current database:
*   **`ArbiterReasonCode`**:
    -   `LowConfidence`: Confidences score is below `min_confidence`.
    -   `ExactDuplicate` / `SemanticDuplicate`: Content is already captured in another active memory.
    -   `ContradictionSupersede`: Conflict detected; candidate confidence exceeds the existing item by a threshold. The existing item is marked `Superseded` and the candidate is saved.
    -   `ContradictionDisputed`: Weak conflict; existing item is marked `Disputed`.
    -   `DeletionRequest`: Explicit user command matches an active memory, marking it `Archived`.
*   **`ArbiterAction`**:
    Generates database operations (`Persist`, `Ignore`, or `Delete`).

### 4. Forgetting Lifecycle (`forgetting.rs`)
Models human memory decay over time:
*   Recency decay score:
    $$\text{score} = \text{initial\_salience} \times e^{-\lambda t}$$
*   Thresholds:
    -   Transitions to `Faded` once the score falls below `FADE_THRESHOLD` (0.3).
    -   Transitions to `Archived` once the score falls below `ARCHIVE_THRESHOLD` (0.1), removing it from conversational recall loops.
*   Pinned memories (`pinned = true`) bypass decay calculations, retaining a score of 1.0.
