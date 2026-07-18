# Streaming Events: Mind Runtime

`ene-runtime`'s actor dispatches every `EneCommand::Run` to the mind streaming pipeline (see [`ene-runtime` API reference § Streaming Dispatch](../api/ene-runtime.md#streaming-dispatch)):

- **Mind** (`streaming_cognitive::run_stream_cognitive`) — delegates prompt composition, recall, affect, and post-turn memory writing to `ene-mind`'s `CognitionEngine`.

The dispatch requires an enabled and initialized store plus an embedding provider. Missing prerequisites return `EneRuntimeError::MindPrerequisite` and emit a failed `Terminal` event; the runtime never falls back to a legacy stream.

Every turn is identified by a [`TurnId`](../api/ene-runtime.md). `run` returns that id (or `RunError::Busy` if a turn is already in flight). Turn-scoped chat events carry the same `turn` field. `Terminal` is emitted after conversation history commit and synchronous `finalize_turn` (affect persist); deferred LLM memory extraction and forgetting may still be in flight.

## Chat `EneEvent` variants

| `EneEvent` variant | Notes |
|---|---|
| `TextDelta { turn, delta }` | Plain-text chunks; emotion / performance markers are stripped. |
| `Performance { turn, cues, source }` | Presentation cues from the Output Arbiter (`PerformanceCue` / `CueSource` in `ene-mind`). Replaces former `SpecialToken` + standalone `Expression` chat events. |
| `ToolCallStart` / `ToolCallResult` | Tool execution lifecycle (when the UI needs them). |
| `PermissionRequired` / `UserInputRequired` | Interactive tool gates. |
| `ContextCompressed { turn, level }` | Thin signal that compression ran; details live on diagnostics. |
| `Terminal { turn, reason }` | Exactly one per `Run`, after history commit and synchronous `finalize_turn` (affect persist). |
| `StatusChanged { status }` | Actor-level Idle / Running / Error. |

### Not on the chat bus

These are **not** chat `EneEvent` variants under API v2:

| Former / diagnostic | Where it lives now |
|---|---|
| `SpecialToken`, standalone `Expression` | Folded into `Performance` (or stripped from text) |
| `SessionSplit` | Compression / split via `diagnostics().manual_split()`; optional thin `ContextCompressed` |
| `PipelinePhase`, `PipelineMetrics`, `TaskProgress` | `handle.diagnostics().subscribe()` |

## Diagnostics

`handle.diagnostics()` returns a concrete `EneDiagnostics` facade. UIs do not implement a diagnostics trait. Use it for snapshots, tool inspection, manual compression/split, and the diagnostic event stream.

## App consumer checklist

Both `ene-cli` (`apps/ene-cli/src/stream.rs`) and `ene-desktop` (`apps/ene-desktop/src/ai_bridge.rs`) match on the minimal chat bus, including `Performance` and `Terminal`. When adding a new UI:

- Break the turn loop on `Terminal` with a matching `turn` — it is the only guaranteed one-per-run completion signal.
- Map `Performance` cues to VRM / CLI display; do not expect `SpecialToken` or standalone `Expression` on the chat bus.
- Treat `ContextCompressed` as an optional thin signal; poll `manual_split()` / diagnostics when you need compression details.

## Related documentation

- [`ene-runtime` API reference](../api/ene-runtime.md) — full `EneEvent` field reference and streaming dispatch
- [API v2 ADR](../architecture/api-v2.md) — locked host / event contracts
- [Session Split and Compression](session-split.md) — why compression is preferred over hard splits
- [Streaming Engine](streaming.md) — actor/handle architecture
- [Cognitive Runtime ADR](../architecture/cognitive-runtime.md)
