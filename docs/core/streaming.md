# Streaming Engine

ene uses an **actor-based message-passing architecture** for streaming LLM conversations with tool calling.

## Architecture

```
Consumer (CLI/Desktop)
    ↓ EneCommand::Run { input }
EneHandle (mpsc channel)
    ↓
EneActor (background tokio task)
    ├── Owns: session, config, tool registry, permissions
    ├── Spawns: stream task (run_stream)
    │     ↓ EneEvent via broadcast channel
    └── Consumer receives events
```

## EneHandle

The public API for consumers. Thread-safe, cloneable.

```rust
pub struct EneHandle {
    cmd_tx: Arc<mpsc::UnboundedSender<EneCommand>>,
    event_tx: broadcast::Sender<EneEvent>,
}
```

### Key Methods

| Method | Description |
|--------|-------------|
| `new()` | Spawns the actor as a background task |
| `run(input)` | Send `EneCommand::Run` (fire-and-forget) |
| `cancel()` | Send `EneCommand::Cancel` |
| `decide_permission(request_id, decision)` | Submit a permission decision for a pending destructive operation |
| `submit_user_input(request_id, response)` | Submit user input response for tools that pause for user interaction |
| `subscribe()` | Get a fresh `EneEventReceiver` for the broadcast event stream |
| `get_snapshot()` | Request read-only state via oneshot |
| `load_character(name)` | Load a character card by name or path |
| `load_config()` | Load config from default paths and apply it |
| `load_config_from(assets_dir, config_path)` | Load config from specified paths and apply it |
| `reconfigure(config)` | Apply new config via oneshot |
| `manual_split()` | Trigger session split via oneshot |
| `list_tools()` | List all registered tool specs via oneshot |
| `call_tool(name, arguments)` | Call a tool directly via oneshot |
| `invalidate_tool_index()` | Invalidate cached tool embeddings for Tool RAG |

### EneEventReceiver

Obtained via `EneHandle::subscribe()`. Wraps the broadcast receiver with ergonomic polling methods.

| Method | Description |
|--------|-------------|
| `try_recv()` | Non-blocking poll (for Bevy ECS) |
| `recv()` | Async receive (for tokio tasks) |

### Lifecycle

- `EneHandle::new()` spawns the actor and returns a handle
- Cloning creates new broadcast receivers (no event loss if done before `run()`)
- `Drop`: sends `Shutdown` only when `Arc::strong_count == 1` (last handle)
- Actor exits when `cmd_rx` returns `None` (all senders dropped)

## EneCommand

Commands sent from consumers to the actor:

```rust
pub enum EneCommand {
    Run { input: String },
    Cancel,
    Shutdown,
    Reconfigure { config: EneConfig, reply: oneshot::Sender<Result<(), EneCoreError>> },
    LoadCharacter { path: String, reply: oneshot::Sender<Result<(), EneCoreError>> },
    PermissionDecision { request_id: RequestId, decision: PermissionDecision },
    UserInputResponse { request_id: RequestId, response: UserInputResponse },
    GetSnapshot { reply: oneshot::Sender<EneStateSnapshot> },
    ManualSplit { reply: oneshot::Sender<Result<SplitResult, EneCoreError>> },
    ListTools { reply: oneshot::Sender<Vec<ToolSpec>> },
    CallTool { name: String, arguments: String, reply: oneshot::Sender<Result<String, ToolError>> },
    InvalidateToolIndex,
}
```

## EneEvent

Events emitted from the actor to all consumers via broadcast channel:

```rust
pub enum EneEvent {
    TextDelta { delta: String },
    SpecialToken { token: String },
    ToolCallStart { name: String, arguments: String },
    ToolCallResult { name: String, result: String },
    PermissionRequired { request_id: RequestId, action: String, target: String, description: String },
    UserInputRequired { request_id: RequestId, prompt: UserInputPrompt },
    TaskProgress { task_id: String, step: usize, total_steps: Option<usize>, description: String },
    SessionSplit { summary: String, reason: SplitReason },
    Done,
    Failed { message: String },
    StatusChanged { status: EneStatus },
}
```

**Note:** `TextDelta` contains only plain text. Special tokens (like `<|emo:name|>`) are already parsed and emitted as separate `SpecialToken` events by the stream task inside `ene-core`.

## Internal Stream Flow (`run_stream`)

The actor spawns a stream task for each `Run` command:

```
Run { input }
  ↓
1. Apply pending split (if any)
2. Check split conditions → spawn split task (if needed)
3. Embed input → pending_embedding
4. Record user input in session
5. Create LLM provider
6. Spawn stream task
  ↓
stream task (run_stream):
  ├── Fetch memory context (summaries + key facts)
  ├── Build messages (system prompt, history, memory, protocol)
  ├── Select relevant tools (Tool RAG)
  ├── Main loop (up to max_tool_call_rounds):
  │     ├── LLM streaming → TextDelta / SpecialToken events
  │     ├── If tool_calls:
  │     │     ├── ToolCallStart event
  │     │     ├── Execute tool (with permission check / user input if needed)
  │     │     ├── ToolCallResult event
  │     │     └── Continue loop
  │     └── If no tool_calls:
  │           ├── Save assistant log
  │           └── Done event
  └── Send updated session back to actor via oneshot
```

## Permission Handling

Destructive tool operations require user approval:

```
Tool execution → PermissionRequired { request_id, action, target, description }
  ↓
Actor sends PermissionRequired event to consumer
  ↓
Consumer displays permission dialog
  ↓
Consumer sends EneCommand::PermissionDecision { request_id, decision }
  ↓
Actor routes decision to the waiting stream task via pending_permissions map
  ↓
Stream task resumes or denies tool execution
```

Permissions are resolved through a shared `Arc<Mutex<HashMap<RequestId, oneshot::Sender<PermissionDecision>>>>` between the actor and the stream task.

## User Input Handling

Some tools (e.g. `utility.question`) need to pause execution and ask the user for interactive input:

```
Tool execution → ToolError::UserInputRequired { request_id, prompt }
  ↓
Stream task emits EneEvent::UserInputRequired { request_id, prompt }
  ↓
Consumer displays interactive dialog (selectable options + free text)
  ↓
Consumer sends EneCommand::UserInputResponse { request_id, response }
  ↓
Actor routes response to the waiting stream task via pending_user_inputs map
  ↓
Stream task injects _user_answers into tool args and re-calls the tool
```

`UserInputResponse` is either `Multi(Vec<MultiAnswer>)` (one answer per question) or `Cancel`. Each `MultiAnswer` is `Selected { option }`, `Answer { text }`, or `Skip`.

## Session Updates

After the stream task completes, it sends the updated `ConversationSession` back to the actor via a oneshot channel. The actor polls for this completion:

- **During streaming:** `tokio::select!` with 100ms sleep checks `stream_session_rx`
- **When idle:** Blocks on `cmd_rx.recv()` (no timer polling)
- On completion: actor updates `self.session` and emits `StatusChanged { status: Idle }`

## Cancellation

`EneCommand::Cancel` triggers:
1. `cancel_token.cancel()` — checked inside the LLM streaming loop
2. `stream_handle.abort()` — kills the tokio task
3. Session state reset to idle

The cancel token is checked inside `while let Some(chunk) = stream.next().await` for immediate response.

## Error Handling

| Error Source | Handling |
|-------------|----------|
| LLM API error | `EneEvent::Failed` + `Done`, stream returns |
| Tool timeout (60s) | Tool error message sent back to LLM |
| Permission denied | Tool error sent back to LLM |
| User input cancelled | Tool error sent back to LLM |
| Max rounds exceeded | `EneEvent::Failed` + `Done` |
| Embedding error | `EneEvent::Failed` + `Done` |
| Broadcast Lagged | Consumer logs warning, continues reading |

## Tool Call Accumulation

Streaming tool calls arrive in chunks that must be accumulated:

```rust
fn accumulate_tool_calls(chunks: &mut Vec<ToolCallChunk>, delta: &[ToolCallChunk])
fn finalize_tool_calls(chunks: Vec<ToolCallChunk>) -> Vec<ToolCall>
```

Each chunk is identified by its `index` field. `function.arguments` strings are concatenated across chunks.

## Screenshot Handling

If a tool result contains `{"type":"screenshot","data":"data:image/png;base64,..."}`, the base64 data is extracted and converted into an `LlmMessage::User` with an image URL for the LLM's next API call.
