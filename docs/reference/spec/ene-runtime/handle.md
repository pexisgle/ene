# `EneHandle` & `EneActor` Lifecycle & Communication Spec

This document details the interface contracts and internal state transitions of `EneHandle`, `EneActor`, and the associated message types.

---

## 1. Data Structures & Enums

### `EneCommand` (Private / Actor Command)
Commands sent to the `EneActor` task via an unbounded `mpsc` channel.
```rust
pub enum EneCommand {
    Run { input: String, turn: TurnId },
    Cancel { turn: TurnId },
    Shutdown,
    PermissionDecision { request_id: RequestId, decision: PermissionDecision },
    UserInputResponse { request_id: RequestId, response: UserInputResponse },
    GetSnapshot { reply: oneshot::Sender<EneStateSnapshot> },
    ManualSplit { reply: oneshot::Sender<Result<SplitResult, EneRuntimeError>> },
    ListTools { reply: oneshot::Sender<Vec<ToolSpec>> },
    CallTool { name: String, arguments: String, turn: Option<TurnId>, reply: oneshot::Sender<Result<String, EneRuntimeError>> },
    InvalidateToolIndex,
    SetCcv3MemoryHash { hash: u64, reply: oneshot::Sender<()> },
    SetCharacter { card: Box<CharacterCardV3>, reply: oneshot::Sender<Result<(), EneRuntimeError>> },
    UpdateProactiveObservation { observation: ene_mind::ProactiveObservation },
    UpdateProactiveSettings { mind: ene_mind::ProactiveConfig },
    UpdateFeatureSettings { settings: Box<FeatureSettingsUpdate> },
    SummarizeScreenImage { width: u32, height: u32, rgb: Vec<u8>, app_label: String, reply: oneshot::Sender<Result<String, String>> },
}
```

### `EneEvent` (Public / Chat Event Bus)
Broadcast events obtained via `EneHandle::subscribe`. All turn-scoped events carry `turn: TurnId` and `origin: TurnOrigin`.
*   `TextDelta { turn, origin, delta }`: Generated chat text delta with markers stripped.
*   `Performance { turn, origin, cues, source }`: Emotion cues for the UI.
*   `ToolCallStart { turn, origin, name, arguments }`: Emitted when the LLM initiates a tool.
*   `ToolCallResult { turn, origin, name, result }`: Emitted when a tool finishes.
*   `PermissionRequired { turn, origin, request_id, action, target, description }`: User authorization request for filesystem writes, deletions, or other destructive actions.
*   `UserInputRequired { turn, origin, request_id, prompt }`: Interaction requests, such as clarifying questions from a tool.
*   `ContextCompressed { turn, origin, level }`: Indicates memory context compression has occurred.
*   `Terminal { turn, origin, reason }`: Final event sent exactly once per turn execution.
*   `StatusChanged { status }`: Emitted when the actor transitions between idle and running states.
*   `TurnStarted { turn, origin }`: Sent after LLM connection and stream launch succeed.

---

## 2. Event Receiver Specification

### `EneEventReceiver`

A subscriber handle that receives streamed conversational and system events.

#### `try_recv`
*   **Signature**: `pub fn try_recv(&mut self) -> Result<EneEvent, broadcast::error::TryRecvError>`
*   **Description**: Performs a non-blocking check for pending events in the broadcast channel.
*   **Return**: Returns the event on success, or a `TryRecvError` indicating the channel is empty or lagged.

#### `recv`
*   **Signature**: `pub async fn recv(&mut self) -> Result<EneEvent, broadcast::error::RecvError>`
*   **Description**: Async wait for the next event to be broadcast.
*   **Return**: Returns the next `EneEvent` or `RecvError`.

---

## 3. Actor Concurrency Guard (`TurnGate`)

Ene's chat streaming runs under a **single-flight** constraint. Calling `run` when a turn is already active returns a `Busy` error. The thread-safe state gate `TurnGate` manages this.

```rust
struct TurnGate {
    busy: AtomicBool,
    active: Mutex<Option<TurnId>>,
}
```

#### `try_begin`
*   **Signature**: `fn try_begin(&self, turn: &TurnId) -> bool`
*   **Description**: Locks the gate using atomic `compare_exchange` and stores the active `TurnId`.
*   **Return**: `true` if lock was successfully acquired; `false` if already busy.

#### `end`
*   **Signature**: `fn end(&self)`
*   **Description**: Clears the active `TurnId` and unlocks the gate. Called by the actor task when finalization wraps up.

#### `matches`
*   **Signature**: `fn matches(&self, turn: &TurnId) -> bool`
*   **Description**: Verifies if the target turn ID matches the currently executing turn (used in cancellation).
*   **Return**: `true` if they match, `false` otherwise.

---

## 4. `EneHandle` Method Specifications

The main entry point for host applications, cheap to clone, and thread-safe.

#### `open`
*   **Signature**: `pub async fn open(config: EneConfig, card: CharacterCardV3) -> Result<Self, EneRuntimeError>`
*   **Initialization Sequence**:
    1. Instantiates command `mpsc` and event/diagnostics broadcast channels.
    2. Registers LLM factory providers (e.g. `OpenAiProviderFactory`).
    3. Validates the `MindConfig`, `StoreConfig`, `ToolConfig`, and `ToolRagConfig` sections.
    4. Spawns the GGUF prefetch task if local weights are required.
    5. Initializes the embedding provider for vector memory/Tool RAG.
    6. Creates the `ConversationSession` and links the embedding provider.
    7. Connects to the SQLite vector store via `init_memory_store`.
    8. Scans and builds active tool instances via `build_tool_registry`.
    9. Starts the background indexer for Tool RAG.
    10. Warms up the character card memories (`warmup_character_memories_ready`) and records the hash.
    11. Spawns the `EneActor` task loop onto the Tokio runtime.

#### `subscribe`
*   **Signature**: `pub fn subscribe(&self) -> EneEventReceiver`
*   **Description**: Creates a new receiver to listen to public runtime events.

#### `diagnostics`
*   **Signature**: `pub const fn diagnostics(&self) -> &crate::diagnostics::EneDiagnostics`
*   **Description**: Returns the diagnostics facade interface.

#### `run`
*   **Signature**: `pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError>`
*   **Process**:
    1. Generates a new `TurnId`.
    2. Checks the `turn_gate`. Returns `RunError::Busy` if active.
    3. Dispatches `EneCommand::Run` to the actor.

#### `cancel`
*   **Signature**: `pub fn cancel(&self, turn: &TurnId) -> Result<(), CancelError>`
*   **Process**:
    1. Checks `turn_gate.matches()`. Returns `CancelError::TurnMismatch` if no matching turn is running.
    2. Dispatches `EneCommand::Cancel` to the actor.

#### `shutdown`
*   **Signature**: `pub async fn shutdown(&self, timeout: std::time::Duration) -> Result<(), ShutdownTimeout>`
*   **Process**: Sends `EneCommand::Shutdown` and waits for the actor's join handle to complete. Returns `ShutdownTimeout` if the timeout is exceeded.

#### `decide_permission`
*   **Signature**: `pub fn decide_permission(&self, request_id: impl Into<RequestId>, decision: PermissionDecision) -> Result<(), ActorDeadError>`
*   **Description**: Submits a permission decision (Allow/Deny) for a pending tool authorization.

#### `submit_user_input`
*   **Signature**: `pub fn submit_user_input(&self, request_id: impl Into<RequestId>, response: UserInputResponse) -> Result<(), ActorDeadError>`
*   **Description**: Submits clarification responses to a tool requesting input.

#### `update_proactive_observation`
*   **Signature**: `pub fn update_proactive_observation(&self, observation: ene_mind::ProactiveObservation) -> Result<(), ActorDeadError>`
*   **Description**: Updates current user activities and app states used in proactive speech triggers.

#### `update_proactive_settings`
*   **Signature**: `pub fn update_proactive_settings(&self, mind: ene_mind::ProactiveConfig) -> Result<(), ActorDeadError>`
*   **Description**: Modifies the cognitive triggers for proactive loops.

#### `update_feature_settings`
*   **Signature**: `pub fn update_feature_settings(&self, mind: ene_mind::MindConfig, store: ene_store::StoreConfig, tools: ene_tool_host::ToolConfig, rag: ToolRagConfig) -> Result<(), ActorDeadError>`
*   **Description**: Live updates settings for AI models, databases, and tools.

#### `summarize_screen_image`
*   **Signature**: `pub async fn summarize_screen_image(&self, width: u32, height: u32, rgb: Vec<u8>, app_label: String) -> Result<String, String>`
*   **Description**: Encodes the raw screen frame and requests a local vision-projection summarization of screen text.

---

## 5. `EneActor` Event Loop Specifications (Private)

`EneActor::run(mut self)` runs as a background task. It processes commands sequentially from the `cmd_rx` channel.

### Run Lifecycle (`EneCommand::Run`)
1. **Streaming Task Spawn**:
   - Creates a turn cancellation token (`CancellationToken`).
   - Spawns the async task `streaming::run_stream` to stream the LLM completion.
   - Sets actor status to `EneStatus::Running` and broadcasts `StatusChanged`.
2. **Task Supervision**:
   - Listens to incoming tool execution requests from the streaming task.
   - Leverages `tokio::task::JoinSet` (`call_tool_tasks`) to run tool subprocesses concurrently without blocking the actor's primary command listener.
3. **Terminal Event Guarantee**:
   - Once the streaming task completes, fails, or triggers cancellation, the actor runs `finalize_turn` to save session history and emotion state.
   - Emits exactly one `EneEvent::Terminal` carrying the termination reason.
   - Unlocks the `turn_gate` via `turn_gate.end()`.

### Internal Functions

#### `run`
*   **Signature**: `async fn run(mut self)`
*   **Description**: The primary event loop. Selects over the command channel, active streams, tool joins, and proactive tick timers.

#### `drain_pending`
*   **Signature**: `async fn drain_pending(&self)`
*   **Description**: Safely cancels and awaits all running background tasks during actor shutdown.

#### `abort_proactive_decision`
*   **Signature**: `fn abort_proactive_decision(&mut self)`
*   **Description**: Aborts any active proactive trigger evaluations.

#### `ensure_proactive_llm`
*   **Signature**: `async fn ensure_proactive_llm(&mut self) -> Result<(), String>`
*   **Description**: Allocates or checks if the local/cloud model provider for proactive decisions is configured and ready.

#### `summarize_screen_rgb`
*   **Signature**: `async fn summarize_screen_rgb(&mut self, width: u32, height: u32, rgb: Vec<u8>, app_label: String, reply: oneshot::Sender<Result<String, String>>)`
*   **Description**: Executes screen layout OCR / image description via local Llama.cpp multimodal helper pipelines.

#### `maybe_spawn_proactive_decision`
*   **Signature**: `async fn maybe_spawn_proactive_decision(&mut self)`
*   **Description**: Evaluates current suppression statuses and schedules a proactive prompt evaluation task if parameters align.

#### `handle_proactive_decision`
*   **Signature**: `async fn handle_proactive_decision(&mut self, result: crate::proactive::ProactiveDecisionResult)`
*   **Description**: Processes outputs from the proactive evaluation and starts a proactive speech turn if a trigger matches.

#### `handle_command`
*   **Signature**: `async fn handle_command(&mut self, cmd: EneCommand) -> bool`
*   **Description**: Decodes and routes commands to their respective internal executors.

#### `start_stream`
*   **Signature**: `fn start_stream(&mut self, user_input: String, turn: TurnId, origin: crate::types::TurnOrigin, record_user_message: bool, allow_tools: bool, runtime_directive: Option<String>, proactive_screen_image: Option<String>, generation_timeout: Option<std::time::Duration>)`
*   **Description**: Bootstraps the cognitive and tool-calling streaming task loop.

#### `create_provider`
*   **Signature**: `fn create_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError>`
*   **Description**: Builds the chat LLM provider instance from active settings.

#### `create_proactive_provider`
*   **Signature**: `fn create_proactive_provider(&self) -> Result<Arc<dyn ene_ai::LlmProvider>, EneRuntimeError>`
*   **Description**: Builds the LLM provider instance for background proactive decisions.

#### `handle_manual_split`
*   **Signature**: `async fn handle_manual_split(&mut self) -> Result<SplitResult, EneRuntimeError>`
*   **Description**: Directly triggers context boundary summarization and compression, saving the new summary and resetting history.

#### `handle_manual_compression`
*   **Signature**: `async fn handle_manual_compression(&mut self) -> Result<SplitResult, EneRuntimeError>`
*   **Description**: Triggers manual context compression without creating a full boundary split.

#### `check_and_perform_split`
*   **Signature**: `fn check_and_perform_split(&mut self, _user_input: &str)`
*   **Description**: Evaluates if the current token budget exceeds split thresholds and queues background splits.

#### `check_and_trigger_compression`
*   **Signature**: `fn check_and_trigger_compression(&mut self)`
*   **Description**: Evaluates if context limits are exceeded and schedules prompt package compression.

#### `apply_pending_compression`
*   **Signature**: `fn apply_pending_compression(&mut self)`
*   **Description**: Applies background split results back to the active session state.

#### `trim_history_after_compression`
*   **Signature**: `fn trim_history_after_compression(&mut self)`
*   **Description**: Slices session history arrays to clean references after context summaries are successfully saved.

#### `spawn_chapter_rollup_if_needed`
*   **Signature**: `fn spawn_chapter_rollup_if_needed(&self)`
*   **Description**: Consolidates multiple scene summaries into chapter-level summaries.

#### `mind_history_entries`
*   **Signature**: `fn mind_history_entries(&self) -> Vec<MindHistoryEntry>`
*   **Description**: Transforms active session history elements into cognitive models.

#### `warmup_character_memories_ready`
*   **Signature**: `async fn warmup_character_memories_ready(config: &EneConfig, session: &ConversationSession) -> Option<u64>`
*   **Description**: Syncs lorebook entries to the database and populates vector caches at startup.

#### `tool_enable_set_changed`
*   **Signature**: `fn tool_enable_set_changed(prev: &ene_tool_host::ToolConfig, next: &ene_tool_host::ToolConfig) -> bool`
*   **Description**: Helper tracking configuration edits to sandbox directories or active tool list overrides.

#### `build_tool_registry`
*   **Signature**: `async fn build_tool_registry(config: &EneConfig, memory_store: Option<Arc<ene_store::MemoryStore>>) -> Result<Arc<dyn ToolRegistry>, EneRuntimeError>`
*   **Description**: Instantiates the sandboxed process registry maps for external tools.

#### `init_embedding`
*   **Signature**: `fn init_embedding(config: &EneConfig) -> Result<Arc<dyn ene_ai::EmbeddingProvider>, ene_ai::EmbeddingError>`
*   **Description**: Resolves configuration keys and constructs the active vector embedder wrapper.

#### `init_memory_store`
*   **Signature**: `async fn init_memory_store(config: &EneConfig, embedder: &dyn ene_ai::EmbeddingProvider) -> Result<Arc<ene_store::MemoryStore>, String>`
*   **Description**: Establishes SQLite connection options, registers sqlite-vec, and migrates schemas.

#### `init_tool_rag`
*   **Signature**: `fn init_tool_rag(config: &EneConfig, embedder: &Arc<dyn ene_ai::EmbeddingProvider>, session: &ConversationSession) -> Result<Option<Arc<ToolRag>>, EneRuntimeError>`
*   **Description**: Initializes the vector indexer and query reranker for Tool RAG.
