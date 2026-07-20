# `CognitionEngine` & Turn Lifecycle Specifications

The `CognitionEngine` is the primary orchestrator facade for the `ene-mind` cognitive pipeline. It coordinates pre-turn analyses, hybrid memory recall, prompt layout formatting, expression rendering resolution, and background memory consolidation.

---

## 1. Struct Definition

### `CognitionEngine` (Public / Struct)
Aggregates all modular sub-components representing the brains of the AI companion:
```rust
pub struct CognitionEngine {
    pub pre_turn: PreTurnAnalyzer,
    pub context: ContextManager,
    pub memory_writer: MemoryWriter,
    pub recall: RecallPlanner,
    pub emotion: EmotionEngine,
    pub character: CharacterProcessor,
    pub prompt_packet: PromptPacket,
    pub output: OutputArbiter,
    pub commitments: CommitmentLedger,
}
```

---

## 2. Core CognitionEngine Methods

#### `new`
*   **Signature**: `pub fn new() -> Self`
*   **Description**: Creates a new `CognitionEngine` instance, initializing each modular sub-component (`PreTurnAnalyzer`, `ContextManager`, `MemoryWriter`, etc.) to its default state.

#### `validate_config`
*   **Signature**: `pub fn validate_config(config: &MindConfig) -> Result<(), CognitionError>`
*   **Description**: Delegates validation to `validate_context_config`. Ensures token sizes and segment boundaries configured in `MindConfig` fit safely within LLM limit envelopes.

#### `sync_character_memories`
*   **Signature**: `pub async fn sync_character_memories(&self, ctx: TurnContext<'_>, previous_hash: Option<u64>) -> Result<(crate::character::CharacterMemorySyncReport, u64), CognitionError>`
*   **Process**:
    1.  Verifies presence of backing databases (`ctx.store`) and vector embedders (`ctx.embedder`).
    2.  Calls `CharacterProcessor::sync_card_memories` to synchronize lorebook items, constant rules, and conversation style cues defined in the `CharacterCardV3`.
    3.  Calculates new card hashes, updating DB registries and returning synchronization reports.

#### `before_turn`
*   **Signature**: `pub async fn before_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **Process**:
    1.  **Emotion Update**:
        -   Loads `AffectState` (PAD coordinates) from `ctx.store`.
        -   Pops and applies pending classifier proposals (`take_pending_affect_proposal`) from the previous turn if their user-turn sequence matches.
        -   Calculates temporal decay based on elapsed time and evaluates new appraisals via `EmotionEngine::update_turn`.
    2.  **Memory Recall**:
        -   Validates vector embeddings and calls `execute_hybrid_recall` using the user input and the embedding.
        -   Combines semantic similarity searches, recency calculations, and emotional filters to select context candidates.
    3.  **Commitment Retrieval**:
        -   Queries up to 16 active user-companion promises from the database via `CommitmentLedger::list_active` and packages them as prompt facts.
    4.  **Assembly**: Compiles outputs into a structured `PreTurnOutput` bundle.

#### `before_proactive_turn`
*   **Signature**: `pub async fn before_proactive_turn(&self, ctx: TurnContext<'_>) -> Result<PreTurnOutput, CognitionError>`
*   **Description**: A lightweight pre-turn loop used when Ene initiates the turn proactively. Since there is no user input text, it bypasses embedding calculations and hybrid search, loading only the active emotional state and active promises.

#### `persist_affect_snapshot`
*   **Signature**: `pub async fn persist_affect_snapshot(store: &MemoryStore, affect: &ene_store::AffectState) -> Result<(), CognitionError>`
*   **Description**: Directly persists changes to the emotional PAD state to SQLite, allowing affect updates to survive stream cancellations or stream pipeline crashes.

#### `compose_prompt_packet`
*   **Signature**: `pub async fn compose_prompt_packet(&self, ctx: TurnContext<'_>, pre: &PreTurnOutput, prefetch: ComposePrefetch) -> Result<ComposedPrompt, CognitionError>`
*   **Process**:
    1.  Compiles the core character personality prompt using `CharacterProcessor::compile_kernel`.
    2.  Resolves conversational style cues from cards or stores.
    3.  Serializes the active emotional state (valence, arousal, mood labels).
    4.  Fetches scene summaries (`load_active_scene_summary`) if not supplied in `prefetch`.
    5.  Slices conversation history to fit limits and passes parameters to `pack_prompt`.
    6.  Triggers token budget compression and session splits if budget counts exceed `ContextBudget` boundaries.
    7.  Converts results into `ComposedPrompt` carrying the text payload and budget metadata.

#### `after_turn`
*   **Signature**: `pub async fn after_turn(&self, store: &MemoryStore, config: &MindConfig, input: PostTurnInput<'_>, providers: crate::memory_writer::MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **Description**: Dispatches deferred background consolidation pipelines (factual memory extraction, vector uploads, and forgets).

#### `finalize_turn_post`
*   **Signature**: `pub async fn finalize_turn_post(&self, store: &MemoryStore, config: &MindConfig, input: &PostTurnInput<'_>) -> Result<(), CognitionError>`
*   **Description**: Executed synchronously immediately when LLM streaming completes. Saves plain dialog lines to text logs and updates affect coordinates in the store.

#### `write_memories_deferred`
*   **Signature**: `pub async fn write_memories_deferred(&self, store: &MemoryStore, config: &MindConfig, input: &crate::lifecycle::OwnedPostTurnInput, providers: crate::memory_writer::MemoryWriteProviders<'_>) -> Result<(), CognitionError>`
*   **Process**:
    1.  Extracts semantic and episodic memory candidates from turn data via `MemoryWriter::write_memories`.
    2.  Evaluates candidates against database records to arbitrate duplicates and contradictions.
    3.  Fades, archives, or decays older memory nodes via `MemoryWriter::apply_forgetting`.

#### `resolve_expression_turn`
*   **Signature**: `pub fn resolve_expression_turn(&self, config: &MindConfig, card: &CharacterCardV3, affect: &ene_store::AffectState, response_text: &str, llm_proposal: Option<&str>, previous_expression: &str, elapsed_since_change: Option<Duration>) -> (crate::output::ExpressionDecision, ene_store::AffectState)`
*   **Description**: Resolves visual morph target blendshape cues. Passes parameters to `OutputArbiter::resolve` to appraise emotional states, punctuation markers, and hysteresis constraints, returning the final expression and the updated state.

---

## 3. Module-Level Helper Functions

#### `build_behavior_contract`
*   **Signature**: `fn build_behavior_contract(card: &CharacterCardV3, user_name: &str) -> Option<String>`
*   **Description**: Extracts creator notes and guides from character cards and expands templates (`expand_cbs_macros`).

#### `pending_to_affect_proposal`
*   **Signature**: `fn pending_to_affect_proposal(pending: ene_store::PendingAffectProposal) -> crate::emotion::AffectProposal`
*   **Description**: Maps database persistent proposals to emotional appraisal structures.

#### `count_user_turns`
*   **Signature**: `pub fn count_user_turns(history: &[crate::lifecycle::HistoryEntry]) -> i64`
*   **Description**: Counts user messages in the active history buffer.

#### `completed_user_turn_at_post_turn`
*   **Signature**: `pub fn completed_user_turn_at_post_turn(history: &[crate::lifecycle::HistoryEntry]) -> i64`
*   **Description**: Helper returning indices for post-turn classification.

#### `build_classifier_context`
*   **Signature**: `pub fn build_classifier_context(history: &[crate::lifecycle::HistoryEntry], current_assistant: &str, affect: &ene_store::AffectState, max_turns: usize) -> ClassifierContext`
*   **Description**: Aggregates history logs and output texts into prompts for background post-turn emotional classifications.
