# Streaming Engine

ene uses an **actor-based message-passing architecture** for streaming LLM conversations with tool calling. The product path is **API v2**: ready `EneHandle::open`, mandatory `TurnId`, single-flight `Busy`, and a minimal chat event bus.

## Architecture

```
Consumer (CLI/Desktop)
    ↓ EneHandle::open(config, card)
EneHandle (mpsc channel)
    ↓ run(input) → TurnId  (or Busy)
EneActor (background tokio task)
    ├── Owns: session, config, tool registry, permissions, mind engine
    ├── Spawns: stream task (run_stream → mind cognitive path)
    │     ↓ EneEvent via broadcast channel (turn-scoped)
    └── Consumer receives events until Terminal { turn, … }
```

## EneHandle

The public API for consumers. Thread-safe, cloneable.

### Key Methods

| Method | Description |
|--------|-------------|
| `open(config, card)` | Async — initializes providers, store, tools, mind, and card **before** returning |
| `run(input)` | Starts a turn; returns `TurnId` or `RunError::Busy` |
| `cancel(turn)` | Cancels only the matching turn (`TurnMismatch` otherwise) |
| `decide_permission(request_id, decision)` | Resolves `PermissionRequired` |
| `submit_user_input(request_id, response)` | Resolves `UserInputRequired` |
| `subscribe()` | Chat `EneEvent` broadcast receiver |
| `diagnostics()` | Concrete facade for snapshot / tools / manual split / diagnostic stream |
| `shutdown(timeout)` | Awaits actor drain |

Config and character file I/O stay in `ene-config` / the host (`ConfigStore`). There is no public unready `new` + multi-step `load_config` / `load_character` on the product path.

### Lifecycle

- `EneHandle::open` spawns the actor and returns only when the handle is ready
- Cloning is cheap; subscribe before `run` if you must not miss early events
- `Drop`: sends `Shutdown` only when the last handle is dropped
- Actor exits when `cmd_rx` returns `None`

## EneEvent (chat bus)

```rust
pub enum EneEvent {
    TextDelta { turn: TurnId, delta: String },
    Performance { turn: TurnId, cues: Vec<PerformanceCue>, source: CueSource },
    ToolCallStart { turn: TurnId, name: String, arguments: String },
    ToolCallResult { turn: TurnId, name: String, result: String },
    PermissionRequired { turn: TurnId, request_id: RequestId, action: String, target: String, description: String },
    UserInputRequired { turn: TurnId, request_id: RequestId, prompt: UserInputPrompt },
    ContextCompressed { turn: TurnId, level: String },
    Terminal { turn: TurnId, reason: TerminalReason },
    StatusChanged { status: EneStatus },
}
```

**Notes:**

- `TextDelta` is plain text only; markers are stripped.
- Presentation cues arrive as `Performance`, not `SpecialToken` / standalone `Expression`.
- `Terminal` is emitted exactly once per `Run`, after `after_turn` completes.
- Pipeline phases / metrics live on `diagnostics().subscribe()`, not the chat bus.

See [Streaming Events](streaming-events.md) for the full consumer checklist.

## Internal Stream Flow (`run_stream`)

The actor validates mind prerequisites (store + embedder), then runs the cognitive path:

```
Run { input, turn }
  ↓
1. before_turn (recall plan + affect)
2. compose_prompt_packet
3. Select relevant tools (Tool RAG)
4. Main loop (up to max_tool_call_rounds):
      ├── LLM streaming → TextDelta / Performance
      ├── If tool_calls → ToolCallStart / execute / ToolCallResult → continue
      └── Else → after_turn (memory write, forgetting, affect persist)
5. Terminal { turn, Done | Failed | Cancelled }
```

Missing store/embedder → `MindPrerequisite` + failed `Terminal`. No legacy streaming fallback.

## Permission Handling

Destructive tool operations require user approval:

```
Tool execution → PermissionRequired { turn, request_id, … }
  ↓
Consumer decide_permission(request_id, AllowOnce | …)
  ↓
Tool resumes or aborts
```

## Related documentation

- [Streaming Events](streaming-events.md)
- [API v2](../architecture/api-v2.md)
- [`ene-runtime` API](../api/ene-runtime.md)
- [Cognitive Runtime ADR](../architecture/cognitive-runtime.md)
