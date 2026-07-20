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

## 2. Actor Concurrency Guard (`TurnGate`)

Ene's chat streaming runs under a **single-flight** constraint. Calling `run` when a turn is already active returns a `Busy` error. The thread-safe state gate `TurnGate` manages this.

```rust
struct TurnGate {
    busy: AtomicBool,
    active: Mutex<Option<TurnId>>,
}
```
*   `try_begin(&self, turn: &TurnId) -> bool`: Locks the gate using atomic `compare_exchange` and stores the active `TurnId`. Returns `false` if already busy.
*   `end(&self)`: Clears the active `TurnId` and unlocks the gate. Called by the actor task when finalization wraps up.
*   `matches(&self, turn: &TurnId) -> bool`: Checks if the target turn ID matches the currently executing turn (used in cancellation).

---

## 3. `EneHandle` Method Specifications

The main entry point for host applications, cheap to clone, and thread-safe.

### Major Methods

#### `open`
```rust
pub async fn open(config: EneConfig, card: CharacterCardV3) -> Result<Self, EneRuntimeError>
```
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

#### `run`
```rust
pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError>
```
*   **Process**:
    1. Generates a new `TurnId`.
    2. Checks the `turn_gate`. Returns `RunError::Busy` if active.
    3. Dispatches `EneCommand::Run` to the actor.

#### `cancel`
```rust
pub fn cancel(&self, turn: &TurnId) -> Result<(), CancelError>
```
*   **Process**:
    1. Checks `turn_gate.matches()`. Returns `CancelError::TurnMismatch` if no matching turn is running.
    2. Dispatches `EneCommand::Cancel` to the actor.

#### `shutdown`
```rust
pub async fn shutdown(&self, timeout: Duration) -> Result<(), ShutdownTimeout>
```
*   **Process**: Sends `EneCommand::Shutdown` and waits for the actor's join handle to complete. Returns `ShutdownTimeout` if the timeout is exceeded.

---

## 4. `EneActor` Event Loop Specifications (Private)

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
