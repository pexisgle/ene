# `ene-runtime` — API Reference

> **Crate**: `ene-runtime` | **Role**: Actor-based host facade & system turn engine

`ene-runtime` is the primary entry point for applications (`ene-cli`, `ene-desktop`) embedding Ene. It coordinates turn execution, prompt composition (`ene-mind`), memory storage (`ene-store`), plugin supervision (`ene-plugin-host`), and DB IPC socket serving.

---

## Key Types & Methods

### `EneHandle`
The thread-safe handle returned when opening Ene:

```rust
pub struct EneHandle { /* ... */ }

impl EneHandle {
    /// Opens the Ene runtime with specified configuration and character card.
    pub async fn open(config: EneConfig, card: CharacterCard) -> Result<Self, EneRuntimeError>;

    /// Initiates a conversation turn (single-flight execution shell).
    pub fn run(&self, input: impl Into<String>) -> Result<TurnId, RunError>;

    /// Cancels an in-flight conversation turn.
    pub fn cancel(&self, turn_id: TurnId) -> Result<(), CancelError>;

    /// Subscribes to the live chat event stream (TokenStream, Performance, Terminal).
    pub fn subscribe(&self) -> broadcast::Receiver<EneEvent>;

    /// Subscribes to the lifecycle event stream (StatusChanged,
    /// PendingCandidateAvailable, ToolBackgroundCompleted).
    pub fn subscribe_lifecycle(&self) -> broadcast::Receiver<LifecycleEvent>;

    /// Takes ownership of the audio (TTS PCM) stream. Single-consumer:
    /// returns `None` on every call after the first.
    pub fn take_audio_stream(&self) -> Option<mpsc::Receiver<AudioChunk>>;

    /// Obtains async diagnostics inspection interface.
    pub fn diagnostics(&self) -> DiagnosticsHandle;

    /// Shuts down the runtime and flushes background memory writers.
    pub async fn shutdown(self) -> Result<(), EneRuntimeError>;
}
```

### Event bus: three channels by traffic class

`ene-runtime`'s events are split across three dedicated channels so a burst
on one never lags or starves consumers of another:

- **Chat bus** (`EneEvent`, via `subscribe`) — a `broadcast` channel
  carrying lightweight, ordered, turn-scoped chat events:

  ```rust
  pub enum EneEvent {
      TurnStarted { turn_id: TurnId },
      TokenStream { chunk: String },
      Performance { cue: PerformanceCue },
      ToolCallStarted { tool_name: String },
      ToolCallFinished { tool_name: String },
      Terminal { turn_id: TurnId, status: TurnStatus },
  }
  ```

- **Audio channel** (`AudioChunk`, via `take_audio_stream`) — a bounded,
  single-consumer `mpsc` channel carrying synthesized TTS PCM. Kept off the
  chat bus because heavyweight PCM payloads would otherwise inflate every
  chat subscriber's `broadcast` buffer.
- **Lifecycle bus** (`LifecycleEvent`, via `subscribe_lifecycle`) — a
  small-capacity `broadcast` channel for turn-independent notifications
  (`StatusChanged`, `PendingCandidateAvailable`, `ToolBackgroundCompleted`).

---

## DB IPC Server (`DbServer`)

`ene-runtime` opens a local Unix Domain Socket (UDS) server that allows stateful tool sub-processes (`ene-plugin-fs`, `ene-plugin-utility`) to execute scoped CRUD operations on `undo.db` and `todo.db` via `ene-plugin-db`.

---

## Related Links
- [System Architecture](../architecture.md)
- [Turns & Sessions](../concepts/turn-and-session.md)
