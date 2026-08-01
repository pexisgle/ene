# `ene-runtime`

> **Crate**: `ene-runtime` | **Role**: Actor-based host facade & turn engine

`ene-runtime` is the primary entry point for applications (`ene-cli`, `ene-desktop`) embedding Ene. It owns `EneHandle`, the thread-safe facade that coordinates turn execution, prompt composition (`ene-mind`), memory storage (`ene-store`), plugin supervision (`ene-plugin-host`), and the tool DB IPC socket server.

---

## Architectural boundaries

- `EneHandle`'s public methods are non-blocking channel sends or oneshot async requests into a single-threaded actor (`handle::actor::TurnActor`); they never touch shared mutable state directly.
- Read-only session/candidate queries and screen-image vision summarization bypass the actor mailbox entirely and talk to `ene-store` / the vision model directly — they do not compete with turn-execution commands for actor throughput.
- The event bus is split into three dedicated channels by traffic class, not one: a `broadcast` chat bus (`EneEvent`), a bounded single-consumer `mpsc` audio channel (`AudioChunk`), and a small-capacity `broadcast` lifecycle bus (`LifecycleEvent`). A burst on one channel cannot lag or starve consumers of another.
- The stable public API v1 contract lives entirely in `public_api` (`PublicApiError`, `PublicChatEvent`, `PublicLifecycleEvent`, `PublicSessionMeta`, `PublicExportedMessage`, `API_VERSION`). No `ene-store` / `ene-mind` / `ene-plugin-proto` type appears in a `Public*` type's fields; internal error enums project into `PublicApiError`'s stable categories via `From` impls, so adding an internal error variant does not break the contract.
- A dead actor is reported uniformly as `PublicApiError::ActorDead` (#408): the actor-control methods on `EneHandle` (permissions, undo, user input, feature updates) and the read-only diagnostics/vision handles all return `PublicApiError` rather than a dedicated actor-dead type. Consumers branch on exactly three error families — `RunError` / `CancelError` / `PublicApiError` — with `RunError::Busy` and `CancelError::TurnMismatch` preserved because callers act on them. `EneRuntimeError` remains only for bootstrap and for diagnostics methods that also surface actor-side failures beyond a dead channel (e.g. `EneRuntimeError::Busy` task admission).
- `message_builder` and `streaming` are intentionally `#[doc(hidden)]` — not part of the API v1 contract, kept visible only for the CLI debug command and integration tests.

## Design rationale

- **Why an actor model**: turn execution needs strictly serialized mutation of shared state (active turn, undo stack, permission grants) without exposing raw locks across an async, potentially multi-consumer API. A single-threaded actor mailbox gives that serialization for free and keeps `EneHandle` cheaply cloneable.
- **Why panic isolation matters here**: `ene-desktop` hosts the GUI, the actor, LLM streaming, and audio in one process. Every dispatched command and background task runs through `catch_unwind`-based isolation so a panic in one command surfaces as a diagnostic event instead of taking down the whole process. This depends on the workspace *not* setting `panic = "abort"` in release profiles — see `docs/architecture.md` §4 for the full mechanism and why that build-configuration detail is load-bearing.
- **Why the event bus was split into three channels**: a single mixed `broadcast` channel let heavyweight `AudioChunk` PCM payloads inflate every chat subscriber's buffer and lag them for reasons unrelated to chat volume. Separating by traffic class removes that coupling.
- **Why read-only queries bypass the actor**: session listing/export/search and vision summarization don't touch turn-execution-critical state, so routing them through the same mailbox as `Run`/`Cancel` would add avoidable head-of-line blocking.
- **Why only side-effect-free tool calls run in parallel**: when the LLM emits several tool calls in one response, executing them strictly one at a time multiplies latency by N (each bounded by `tools.timeout_ms`) and stalls `TextDelta`. But most correctness invariants — permission/user-input prompts, the undo stack, same-resource writes, and `ToolCallStart`/`ToolCallResult` / `ToolResultSummary` ordering — depend on a deterministic sequence. The resolution is a two-phase loop: tools whose `ToolSpec` declares `side_effects: ReadOnly` (and which are not background-capable) are dispatched concurrently up to `plugins.parallel_tool_calls_max`, capturing only their raw results; everything is then finalized in the original `tool_calls` order, emitting events, resolving any prompt, and updating the undo stack sequentially. A parallel-classified tool that unexpectedly returns `PermissionRequired`/`UserInputRequired` falls back to sequential resolution. The classification is fail-closed: a tool that does not declare `ReadOnly` side effects (including `system.search_tools`, which needs the Tool RAG) is never parallelized, and `parallel_tool_calls_max: 0` restores the old fully-sequential behavior.
- **Why the command mailbox stays unbounded**: Stage 8 bounded the actor's five background `JoinSet`s, but the command channel that feeds them was left unbounded on purpose (#404). The channel is shared by external consumers, the actor's own background tasks (which feed `PluginHostReconfigured` back through it), and the last-handle drop guard (which sends `Shutdown` from a synchronous `Drop`). A bounded channel with `try_send` backpressure could silently drop the shutdown command or an internal reconfiguration — correctness bugs, not graceful degradation. The genuinely expensive work (tool calls, searches, deferred pollers, GGUF loads) is bounded where it matters, at `JoinSet` admission, failing fast with `EneRuntimeError::Busy`. The only externally floodable command — `UpdateProactiveObservation` — is rate-limited at its source (screen-capture cadence), so the realistic flood vector is bounded upstream, not by the mailbox.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-runtime --open
```

Start at `EneHandle`, then `handle::EneEvent` and `handle::LifecycleEvent` for the event bus.

---

## Related
- [System Architecture](../architecture.md)
- [Turns & Sessions](../concepts/turn-and-session.md)
