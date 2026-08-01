# `ene-runtime`

> **Crate**: `ene-runtime` | **Role**: Actor-based host facade & turn engine

`ene-runtime` is the primary entry point for applications (`ene-cli`, `ene-desktop`) embedding Ene. It owns `EneHandle`, the thread-safe facade that coordinates turn execution, prompt composition (`ene-mind`), memory storage (`ene-store`), plugin supervision (`ene-plugin-host`), and the tool DB IPC socket server.

---

## Architectural boundaries

- `EneHandle`'s public methods are non-blocking channel sends or oneshot async requests into a single-threaded actor (`handle::actor::TurnActor`); they never touch shared mutable state directly.
- Read-only session/candidate queries and screen-image vision summarization bypass the actor mailbox entirely and talk to `ene-store` / the vision model directly — they do not compete with turn-execution commands for actor throughput.
- Small per-frame state is mirrored into mailbox-free shared slots: `EneHandle::card_name()`, `session_id()`, `session_started_at()`, `turn_count()`, `config()`, and `character_card()` each take one `parking_lot` lock (or an atomic) and read a slot the actor keeps in sync at the mutation point (session split, `SetCharacter`, per-turn bookkeeping, feature-settings updates) — safe to call from egui immediate mode, never queueing behind an in-flight `Run` turn. Only the large history payload stays mailbox-based, via the dedicated `EneHandle::history()`.
- Tool operations (`list` / `search` / `call` / `invalidate`) have their own handle, `EneHandle::tools()` (#406), but deliberately stay on the actor mailbox: tool calls and searches are admission-capped there (Stage 8, `EneRuntimeError::Busy`) and the registry is actor-owned state, swapped on plugin-host reconfiguration. Unlike the read-only handles, `ToolHandle` is an API-shape split, not a transport bypass.
- The control surface lives on `EneHandle` itself, not the diagnostics facade: `set_character` (character-card swap) and `compress_context` (manual compression-only pass, returning `ene_mind::CompressionResult` — the session id is unchanged). `EneHandle::diagnostics()` is strictly observability: pipeline detail, provider health, memory/journal inspection, and bulk actor snapshots (`get_snapshot`) for the CLI's one-shot commands.
- The memory surface (`EneDiagnostics::memory` → `MemoryHandle`) is the one place the diagnostics facade mutates memory: pin/status/restore/forget, commitment lifecycle (`complete_commitment` / `cancel_commitment`), pending-write inspection and drain, and store backup/integrity diagnostics. It does **not** expose the raw `MemoryStore` — the former `store()` backdoor was removed, so consumers cannot reach the DB handle directly. Commitment writes are actor-safe because `ene_mind::commitments::CommitmentLedger` is stateless: every access re-reads the `commitments` table, so a UI-side write can never desync an actor-side cache.
- The event bus is split into three dedicated channels by traffic class, not one: a `broadcast` chat bus (`EneEvent`), a bounded single-consumer `mpsc` audio channel (`AudioChunk`), and a small-capacity `broadcast` lifecycle bus (`LifecycleEvent`). A burst on one channel cannot lag or starve consumers of another.
- The stable public API v1 contract lives entirely in `public_api` (`PublicApiError`, `PublicChatEvent`, `PublicLifecycleEvent`, `PublicSessionMeta`, `PublicExportedMessage`, `API_VERSION`). No `ene-store` / `ene-mind` / `ene-plugin-proto` type appears in a `Public*` type's fields; internal error enums project into `PublicApiError`'s stable categories via `From` impls, so adding an internal error variant does not break the contract.
- A dead actor is reported uniformly as `PublicApiError::ActorDead` (#408): the actor-control methods on `EneHandle` (permissions, undo, user input, feature updates) and the read-only diagnostics/vision handles all return `PublicApiError` rather than a dedicated actor-dead type. Consumers branch on exactly three error families — `RunError` / `CancelError` / `PublicApiError` — with `RunError::Busy` and `CancelError::TurnMismatch` preserved because callers act on them. `EneRuntimeError` remains for bootstrap and the control/tool-handle methods that also surface actor-side failures (e.g. `Busy` admission, `SplitNotNeeded`).
- `message_builder` and `streaming` are intentionally `#[doc(hidden)]` — not part of the API v1 contract, kept visible only for the CLI debug command and integration tests.

## Design rationale

- **Why an actor model**: turn execution needs strictly serialized mutation of shared state (active turn, undo stack, permission grants) without exposing raw locks across an async, potentially multi-consumer API. A single-threaded actor mailbox gives that serialization for free and keeps `EneHandle` cheaply cloneable.
- **Why panic isolation matters here**: `ene-desktop` hosts the GUI, the actor, LLM streaming, and audio in one process. Every dispatched command and background task runs through `catch_unwind`-based isolation so a panic in one command surfaces as a diagnostic event instead of taking down the whole process. This depends on the workspace *not* setting `panic = "abort"` in release profiles — see `docs/architecture.md` §4 for the full mechanism and why that build-configuration detail is load-bearing.
- **Why the event bus was split into three channels**: a single mixed `broadcast` channel let heavyweight `AudioChunk` PCM payloads inflate every chat subscriber's buffer and lag them for reasons unrelated to chat volume. Separating by traffic class removes that coupling.
- **Why read-only queries bypass the actor**: session listing/export/search and vision summarization don't touch turn-execution-critical state, so routing them through the same mailbox as `Run`/`Cancel` would add avoidable head-of-line blocking.
- **Why small state reads are mailbox-free**: the desktop polls state every frame (egui immediate mode), and a mailbox round-trip per frame would queue behind in-flight turns and starve under load. `EneHandle` therefore mirrors the small snapshot-ish surface (card name, session id, turn count, config, card) into shared slots the actor writes at the mutation points; the card-name slot is the precedent. The snapshot itself (`EneDiagnostics::get_snapshot`) remains for one-shot CLI bulk reads and no longer carries the memory handle — memory access is `EneDiagnostics::memory()`.
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
