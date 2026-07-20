# Conversational Streaming & Actor Dispatch Specifications

This document defines the technical specifications of the conversational streaming pipelines, including LLM text streaming, tool calling execution, approval security gates, and cognitive runtime integration.

---

## 1. Data Structures

### `PermissionDecision` (Public / Enum)
Represents the user's decision for a destructive tool invocation:
*   `AllowOnce`: Execute this single call.
*   `AllowSession`: Authorize this operation for the rest of the session.
*   `Deny`: Abort the tool call and return an authorization error to the LLM.

### `UserInputResponse` (Public / Enum)
Represents responses to interactive user input requests:
*   `Multi(Vec<MultiAnswer>)`: Vector of answers to the questions.
*   `Cancel`: User cancels the prompt.

### `StreamContext` (Private / Execution Context)
Configuration packet sent to spawn the streaming loop:
```rust
pub struct StreamContext {
    pub config: Arc<EneConfig>,
    pub session: ConversationSession,
    pub user_input: String,
    pub embedder: Option<Arc<dyn EmbeddingProvider>>,
    pub registry: Arc<dyn ToolRegistry>,
    pub tool_rag: Option<Arc<ToolRag>>,
    pub provider: Arc<dyn LlmProvider>,
    pub event_tx: broadcast::Sender<EneEvent>,
    pub diag_tx: broadcast::Sender<DiagnosticEvent>,
    pub cancel_token: CancellationToken,
    pub pending_permissions: Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
    pub pending_user_inputs: Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
    pub terminal_emitted: Arc<AtomicBool>,
    pub turn: TurnId,
    pub origin: TurnOrigin,
    pub allow_tools: bool,
    pub runtime_directive: Option<String>,
    pub proactive_screen_image: Option<String>,
    pub generation_timeout: Option<Duration>,
}
```

> [!NOTE]
> Background task handles (`classifier_tx`, `memory_writer_tx`) have been removed from `StreamContext`. Background consolidation tasks spawned by `CognitionEngine::finalize_turn` are tracked in `EneActor`'s `JoinSet`, decoupling task lifecycle management from the stream context packet.



---

## 2. Standard Chat Loop Functions (`streaming.rs`)

`run_stream` processes conversational streaming without long-term memory or emotion model appraisal. It is strictly limited to basic context and tool executions, keeping it decoupled from the complex cognitive pipelines managed in `streaming_cognitive.rs`.

#### `run_stream`
*   **Signature**: `pub async fn run_stream(ctx: StreamContext) -> StreamOutcome`
*   **Process**:
    1.  Loads candidate tools via `select_relevant_tools` (utilizing Tool RAG if configured).
    2.  Formats messages using `build_messages`.
    3.  Calls `provider.stream_chat`.
    4.  Processes tokens. Broadcasts text segments via `TextDelta`.
    5.  Collects tool chunks (`accumulate_tool_calls`), pauses the stream, and runs them via `perform_tool_executions`.
    6.  Closes the loop and broadcasts `Terminal(Done)` via `stream_finish`.

#### `stream_finish`
*   **Signature**: `pub(crate) fn stream_finish(session: ene_mind::ConversationSession, event_tx: &broadcast::Sender<EneEvent>, guard: &AtomicBool, turn: &TurnId, origin: TurnOrigin, reason: TerminalReason) -> StreamOutcome`
*   **Description**: Finalizes stream resources, registers session statistics, and emits the final turn terminal state if it hasn't been broadcast yet.

#### `emit_terminal`
*   **Signature**: `pub(crate) fn emit_terminal(event_tx: &broadcast::Sender<EneEvent>, guard: &AtomicBool, turn: &TurnId, origin: TurnOrigin, reason: TerminalReason)`
*   **Description**: Thread-safe helper that sets the status to idle, clears turn flags, and broadcasts exactly one `EneEvent::Terminal` payload.

#### `select_relevant_tools`
*   **Signature**: `pub(crate) async fn select_relevant_tools(registry: &dyn ene_tool_host::ToolRegistry, tool_rag: Option<&ToolRag>, user_input: &str, query_embedding: Option<&[f32]>, tool_calling_enabled: bool) -> Vec<ene_tool_proto::ToolSpec>`
*   **Description**: Filters tools to fit model contexts. Uses RAG vector similarity rankings on embedding models if enabled, otherwise returns the default toolset.

#### `perform_tool_executions`
*   **Signature**:
    ```rust
    pub(crate) async fn perform_tool_executions(
        registry: &dyn ene_tool_host::ToolRegistry,
        session_id: &str,
        tool_calls: Vec<LlmToolCall>,
        assistant_content: &str,
        event_tx: &broadcast::Sender<EneEvent>,
        turn: &crate::types::TurnId,
        origin: TurnOrigin,
        pending_permissions: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>,
        pending_user_inputs: &Arc<Mutex<HashMap<RequestId, oneshot::Sender<UserInputResponse>>>>,
        timeout_ms: u64,
        max_summary_chars: usize,
    ) -> Result<ToolExecutionOutput, crate::error::EneRuntimeError>
    ```
*   **Process**:
    -   Validates tool specifications from the registry.
    -   **Sandbox & Permissions**: If the tool requests `sandbox = false` or has side-effects, it allocates a `RequestId`, triggers `PermissionRequired`, and awaits user feedback via `pending_permissions`.
    -   **Interactive Inputs**: If a tool prompts for user questions, it fires `UserInputRequired` and yields until a response is received.
    -   Executes the target binary and broadcasts `ToolCallResult`. Feeds the result back to the LLM context.

#### `accumulate_tool_calls`
*   **Signature**: `pub(crate) fn accumulate_tool_calls(current_tool_calls: &mut Vec<LlmToolCallChunk>, tool_calls_delta: &[LlmToolCallChunk])`
*   **Description**: Merges incoming streaming delta chunks (index offsets and string slices) into candidate tool arguments.

#### `finalize_tool_calls`
*   **Signature**: `pub(crate) fn finalize_tool_calls(current_tool_calls: Vec<LlmToolCallChunk>) -> Vec<LlmToolCall>`
*   **Description**: Packages gathered delta chunks into validated, query-ready `LlmToolCall` models.

#### `extract_screenshot`
*   **Signature**: `fn extract_screenshot(result: &str) -> (String, Option<String>)`
*   **Description**: Scans tool outcome bodies for encoded screenshot marker payloads.

#### `inject_user_answers`
*   **Signature**: `fn inject_user_answers(args_json: &str, answers: &[MultiAnswer]) -> String`
*   **Description**: Merges custom user-resolved answers back into JSON parameter trees of interactive tools.

---

## 3. Cognitive Chat Loop Functions (`streaming_cognitive.rs`)

`run_stream_cognitive` integrates hybrid memory recall, emotional appraisal, output presentation, and background consolidation.

#### `build_turn_context`
*   **Signature**:
    ```rust
    fn build_turn_context<'a>(
        mind: &'a MindConfig,
        card: &'a ene_config::CharacterCardV3,
        card_name: &'a str,
        user_name: &'a str,
        session_id: &'a str,
        user_input: &'a str,
        history: &'a [HistoryEntry],
        mem_store: &'a Option<std::sync::Arc<ene_store::MemoryStore>>,
        query_embedding: Option<&'a [f32]>,
        embedder: Option<&'a std::sync::Arc<dyn ene_ai::EmbeddingProvider>>,
        provider: &std::sync::Arc<dyn ene_ai::LlmProvider>,
        post_history_block: Option<&'a str>,
    ) -> TurnContext<'a>
    ```
*   **Description**: Compiles prompt configurations, vector settings, and user input references into a unified context token payload.

#### `apply_proactive_prompt`
*   **Signature**: `fn apply_proactive_prompt(messages: &mut Vec<LlmMessage>, directive: Option<&str>, screen_image_data_uri: Option<&str>)`
*   **Description**: Modifies conversation history prompts for proactive turns by injecting system guidelines and user screen screenshots.

#### `run_stream_cognitive`
*   **Signature**: `pub async fn run_stream_cognitive(ctx: StreamContext) -> StreamOutcome`
*   **Process (5-Phase Pipeline)**:
    1.  **Phase A: Embedding & Sync**:
        Computes user query embeddings via `embed_query`. Synchronizes character memories.
    2.  **Phase B: Pre-Turn (before_turn)**:
        Invokes `engine.before_turn` (appraisal, hybrid recalls, commitments) to construct `PreTurnOutput`.
    3.  **Phase C: Prompt Composition (compose_prompt_packet)**:
        Composes and packs prompt sections. Compresses session data if token limits are exceeded.
    4.  **Phase D: LLM Stream & Tool Execution**:
        Streams LLM completions, resolves expression tags, and dispatches tool execution frames.
    5.  **Phase E: Finalization & Consolidation (finalize_turn)**:
        Saves chat logs and emotions. Spawns the background affect classifier and memory consolidator task loops.
