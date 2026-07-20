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

#### `write_memories`
*   **Signature**: `pub async fn write_memories(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>, providers: MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **Process**:
    1.  **Deterministic Extraction**: Runs regex scans and tool grounding extractions.
    2.  **LLM Extraction**: Prompts the LLM for memory candidates. Falls back to deterministic candidates on empty replies or connection timeouts.
    3.  **Arbitration**: Evaluates candidates against active DB records for duplicates and contradictions via vector matching.
    4.  **Embed & Save**: Generates embeddings for approved candidates and persists them to SQLite.

#### `apply_forgetting`
*   **Signature**: `pub async fn apply_forgetting(store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **Description**: Invokes the decay transaction pipeline (`ForgettingLifecycle::apply`) to transition aged memories from `Active` to `Faded` or `Archived` states.

#### `finalize_turn`
*   **Signature**: `pub async fn finalize_turn(store: &MemoryStore, _config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **Description**: Synchronously writes plain text conversation logs to `conversation_logs` and marks matching commitments resolved before the turn wraps up.

#### `after_turn`
*   **Signature**: `pub async fn after_turn(store: &MemoryStore, config: &MindConfig, input: PostTurnInput<'_>, providers: MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **Description**: Dispatches deferred consolidation pipelines.

#### `build_semantic_matches`
*   **Signature**: `async fn build_semantic_matches(store: &MemoryStore, embedder: Option<&dyn EmbeddingProvider>, config: &crate::config::MindMemoryConfig, character_id: &str, user_id: &str, candidates: &[candidate::MemoryCandidate], similarity_threshold: f32) -> Result<HashMap<usize, Vec<SemanticMatch>>, CognitionError>`
*   **Description**: Runs batch vector similarity queries on candidate embeddings to locate overlapping DB records.

#### `sanitize_ref`
*   **Signature**: `fn sanitize_ref(raw: &str) -> String`
*   **Description**: Strips whitespace and normalizes reference IDs.

#### `locale_from_classifier_language`
*   **Signature**: `const fn locale_from_classifier_language(lang: &str) -> candidate::Locale`
*   **Description**: Maps configurations to extraction localization modes (`Locale::Ja` / `Locale::En`).

#### `record_arbiter_outcomes`
*   **Signature**: `fn record_arbiter_outcomes(input: &PostTurnInput<'_>, applied: &[crate::memory_writer::AppliedDecision], summary: &mut ArbiterOutcomeSummary)`
*   **Description**: Logs decisions to metrics.

---

## 3. Deterministic Extractions (`deterministic.rs` & `tool_grounding.rs`)

#### `extract`
*   **Signature**: `pub fn extract(turn: &TurnInput<'_>, locale: Locale, min_confidence: f32) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: Parses turn logs for explicit directives (e.g. "remember that...") using language regex filters.

#### `extract_with_tool_grounding`
*   **Signature**: `pub fn extract_with_tool_grounding(turn: &TurnInput<'_>, locale: Locale, min_confidence: f32, tool_grounding_cfg: &ToolGroundingConfig) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: Integrates explicit remembers and tool execution candidates.

#### `ja_explicit_remember` / `en_explicit_remember`
*   **Signature**: `fn ja_explicit_remember(user_msg: &str, _asst_msg: &str, _tool_results: &[ToolResultSummary]) -> Option<MemoryCandidate>` (same pattern for EN)
*   **Description**: Regex extractors searching for explicit user instructions to memorize details.

#### `ja_forget_request` / `en_forget_request`
*   **Signature**: `fn ja_forget_request(user_msg: &str, _asst_msg: &str, _tool_results: &[ToolResultSummary]) -> Option<MemoryCandidate>` (same pattern for EN)
*   **Description**: Scans for commands asking to erase/forget details.

#### `summarize_tool_result`
*   **Signature**: `pub fn summarize_tool_result(tool_name: &str, raw_output: &str, success: bool, max_summary_chars: usize) -> ToolResultSummary`
*   **Description**: Summarizes tool execution outputs for memory storage.

#### `extract_tool_candidates`
*   **Signature**: `pub fn extract_tool_candidates(tool_results: &[ToolResultSummary], cfg: &ToolGroundingConfig) -> Vec<MemoryCandidate>`
*   **Description**: Generates procedural or factual memory candidates based on tool execution logs (e.g. noting file paths written).

#### `normalize_tool_output`
*   **Signature**: `fn normalize_tool_output(raw_output: &str) -> String`
*   **Description**: Cleans output characters.

#### `is_screenshot_payload`
*   **Signature**: `fn is_screenshot_payload(result: &str) -> bool`
*   **Description**: Detects visual bitmap blocks in result payloads.

---

## 4. LLM Extractions (`llm.rs`)

#### `extract`
*   **Signature**: `pub async fn extract(provider: &dyn LlmProvider, turn: &TurnInput<'_>, locale: Locale) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: Executes structured LLM prompts to extract memory candidates from turn dialogs.

#### `extract_with_timeout`
*   **Signature**: `pub async fn extract_with_timeout(provider: &dyn LlmProvider, turn: &TurnInput<'_>, locale: Locale, timeout_secs: u64, pattern_hints: &[MemoryCandidate]) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: Wraps LLM extractions in timeout safety limits.

#### `format_pattern_hints`
*   **Signature**: `fn format_pattern_hints(hints: &[MemoryCandidate]) -> String`
*   **Description**: Pre-pends pattern guides to structure assistant inputs.

#### `build_conversation_text`
*   **Signature**: `fn build_conversation_text(turn: &TurnInput<'_>) -> String`
*   **Description**: Normalizes user and assistant turns for prompts.

#### `extraction_schema`
*   **Signature**: `fn extraction_schema() -> serde_json::Value`
*   **Description**: Builds the schema defining output structures.

#### `parse_candidates_json`
*   **Signature**: `fn parse_candidates_json(raw: &str, locale: Locale) -> Result<Vec<MemoryCandidate>, CognitionError>`
*   **Description**: Deserializes JSON arrays into candidates.

#### `raw_to_candidate`
*   **Signature**: `fn raw_to_candidate(raw: RawCandidate, locale: Locale) -> MemoryCandidate`
*   **Description**: Validates candidate parameters.

#### `locale_mismatch`
*   **Signature**: `fn locale_mismatch(text: &str, locale: Locale) -> bool`
*   **Description**: Triggers alignment validation checks.

---

## 5. Duplicate Arbitration (`arbiter.rs`)

Validates and deduplicates candidate items against the current database:

#### `evaluate_all`
*   **Signature**: `pub fn evaluate_all(candidates: &[MemoryCandidate], existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> Vec<CandidateDecision>`
*   **Description**: Iterates through candidates, validating structures and resolving database duplications or contradictions.

#### `evaluate_one`
*   **Signature**: `pub(crate) fn evaluate_one(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>, semantic_matches: &[SemanticMatch]) -> CandidateDecision`
*   **Description**: Checks confidence limits, runs duplication filters, and evaluates contradictions.

#### `evaluate_deletion`
*   **Signature**: `fn evaluate_deletion(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> CandidateDecision`
*   **Description**: Identifies and schedules archives for items matching deletion commands.

#### `validate_candidate`
*   **Signature**: `fn validate_candidate(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> Option<ArbiterReason>`
*   **Description**: Confirms saliences, quotes, and constraints.

#### `check_semantic_matches`
*   **Signature**: `fn check_semantic_matches(candidate: &MemoryCandidate, semantic_matches: &[SemanticMatch], ctx: &ArbiterContext<'_>, existing: &[MemoryItem]) -> Option<CandidateDecision>`
*   **Description**: Compares candidate scores with overlapping records to detect duplicates.

#### `check_contradiction`
*   **Signature**: `fn check_contradiction(candidate: &MemoryCandidate, existing: &[MemoryItem], ctx: &ArbiterContext<'_>) -> Option<CandidateDecision>`
*   **Description**: Detects if candidates contradict active database values.

#### `contradiction_decision`
*   **Signature**: `fn contradiction_decision(candidate: &MemoryCandidate, existing: &MemoryItem, ctx: &ArbiterContext<'_>, supersede_code: ArbiterReasonCode, dispute_code: ArbiterReasonCode, ask_code: ArbiterReasonCode, detail: String) -> Option<CandidateDecision>`
*   **Description**: Resolves contradictions: supersedes, disputes, or flags for clarification.

#### `apply_decisions`
*   **Signature**: `pub async fn apply_decisions(store: &MemoryStore, decisions: &[CandidateDecision]) -> Result<Vec<AppliedDecision>, CognitionError>`
*   **Description**: Commits final decisions to the store in a single transaction.

#### `apply_one`
*   **Signature**: `async fn apply_one(store: &MemoryStore, decision: &CandidateDecision) -> Result<AppliedDecision, CognitionError>`
*   **Description**: Performs single database actions (inserts, updates, or deletes).

#### `candidate_to_new_item`
*   **Signature**: `fn candidate_to_new_item(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> NewMemoryItem`
*   **Description**: Formats memory candidates into database records.

#### `passes_validation`
*   **Signature**: `fn passes_validation(candidate: &MemoryCandidate, ctx: &ArbiterContext<'_>) -> bool`
*   **Description**: Verifies candidate fields.

#### `normalize_text`
*   **Signature**: `fn normalize_text(s: &str) -> String`
*   **Description**: Normalizes strings for matching.

#### `dedup_key`
*   **Signature**: `fn dedup_key(candidate: &MemoryCandidate) -> (MemoryKind, String)`
*   **Description**: Generates unique mapping keys.

#### `source_quote_valid`
*   **Signature**: `fn source_quote_valid(candidate: &MemoryCandidate, turn: &TurnInput<'_>) -> bool`
*   **Description**: Verifies if source quotes reside inside dialog transcript boundaries.

#### `find_exact_duplicate`
*   **Signature**: `fn find_exact_duplicate(candidate: &MemoryCandidate, existing: &[MemoryItem]) -> Option<i64>`
*   **Description**: Matches title/content strings.

#### `find_deletion_targets`
*   **Signature**: `fn find_deletion_targets(target: &str, existing: &[MemoryItem]) -> Vec<i64>`
*   **Description**: Identifies records matching deletion commands.
