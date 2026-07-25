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

    /// Obtains async diagnostics inspection interface.
    pub fn diagnostics(&self) -> DiagnosticsHandle;

    /// Shuts down the runtime and flushes background memory writers.
    pub async fn shutdown(self) -> Result<(), EneRuntimeError>;
}
```

### `EneEvent`
Live chat events broadcast during a turn:

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

---

## DB IPC Server (`DbServer`)

`ene-runtime` opens a local Unix Domain Socket (UDS) server that allows stateful tool sub-processes (`ene-plugin-fs`, `ene-plugin-utility`) to execute scoped CRUD operations on `undo.db` and `todo.db` via `ene-tool-db`.

---

## Related Links
- [System Architecture](../architecture.md)
- [Turns & Sessions](../concepts/turn-and-session.md)
