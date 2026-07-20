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
    pub config: EneConfig,
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
    pub classifier_tx: mpsc::UnboundedSender<JoinHandle<()>>,
    pub memory_writer_tx: mpsc::UnboundedSender<JoinHandle<()>>,
}
```

---

## 2. Standard Chat Loop (`streaming.rs`)

`run_stream` processes conversational streaming without long-term memory or emotion model appraisal.

### Core Functions

#### `run_stream`
*   **Signature**: `pub async fn run_stream(ctx: StreamContext) -> StreamOutcome`
*   **Process**:
    1.  Loads candidate tools via `select_relevant_tools` (utilizing Tool RAG if configured).
    2.  Formats messages using `build_messages`.
    3.  Calls `provider.stream_chat`.
    4.  Processes tokens. Broadcasts text segments via `TextDelta`.
    5.  Collects tool chunks (`accumulate_tool_calls`), pauses the stream, and runs them via `perform_tool_executions`.
    6.  Closes the loop and broadcasts `Terminal(Done)` via `stream_finish`.

#### `perform_tool_executions`
*   **Signature**:
    ```rust
    pub(crate) async fn perform_tool_executions(
        calls: Vec<LlmToolCall>,
        // ... (truncated parameters)
    ) -> Result<ToolExecutionOutput, ToolError>
    ```
*   **Process**:
    -   Validates tool specifications from the registry.
    -   **Sandbox & Permissions**: If the tool requests `sandbox = false` or has side-effects, it allocates a `RequestId`, triggers `PermissionRequired`, and awaits user feedback via `pending_permissions`.
    -   **Interactive Inputs**: If a tool prompts for user questions, it fires `UserInputRequired` and yields until a response is received.
    -   Executes the target binary and broadcasts `ToolCallResult`. Feeds the result back to the LLM context.

---

## 3. Cognitive Chat Loop (`streaming_cognitive.rs`)

`run_stream_cognitive` integrates hybrid memory recall, emotional appraisal, output presentation, and background consolidation.

### Turn Lifecycle (5-Phase Pipeline)

#### 1. Phase A: Embedding & Sync
*   Computes user query embeddings via `embed_query`.
*   If the character card memory hash (`ccv3_memory_hash`) mismatches the struct value, the engine synchronizes lorebook entries and style declarations (`engine.sync_character_memories`) synchronously.

#### 2. Phase B: Pre-Turn (before_turn)
*   Invokes `engine.before_turn` to run in parallel:
    -   Reloading emotion state and computing appraisal parameters.
    -   Retrieving episodic, semantic, and lorebook memories from SQLite.
    -   Running Tool RAG retrieval.
*   Aggregates results into `PreTurnOutput`.

#### 3. Phase C: Prompt Composition (compose_prompt_packet)
*   Invokes `engine.compose_prompt_packet` to pack sections within `ContextBudget` tokens.
*   Triggers session splits (`session_split`) and summarization if the budget is exceeded.
*   Orders system prompt sections, memories, emotion cues, conversation history, and user input.

#### 4. Phase D: LLM Stream & Tool Execution
*   Launches LLM chat completion.
*   Parses inline tags like `<|perf:expr=NAME|>` and routes them as `Performance` events while stripping them from conversation logs and user output.
*   Executes tool calls via `perform_tool_executions` and feeds outputs back into the LLM context.

#### 5. Phase E: Finalization & Consolidation (finalize_turn)
*   Runs `engine.finalize_turn` to commit conversation logs, update commitments, and save emotion states.
*   **Background Tasks**: To keep latency low, the following pipelines are spawned asynchronously *after* emitting the `Terminal` event:
    -   **Affect Classifier (`spawn_affect_classifier`)**: Categorizes emotional responses and appends proposals for the next turn.
    -   **Memory Consolidation (`spawn_memory_writer`)**: Parses the history using `MemoryArbiter` to extract long-term semantic facts and runs memory decay calculations.
