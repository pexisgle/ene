# Streaming Events: Mind Runtime

`ene-runtime`'s actor dispatches every `EneCommand::Run` to the mind streaming pipeline (see [`ene-runtime` API reference § Streaming Dispatch](../api/ene-runtime.md#streaming-dispatch)):

- **Mind** (`streaming_cognitive::run_stream_cognitive`) — delegates prompt composition, recall, affect, and post-turn memory writing to `ene-mind`'s `CognitionEngine`.

The dispatch requires an enabled and initialized store plus an embedding provider. Missing prerequisites return `EneRuntimeError::MindPrerequisite` and emit a failed `Terminal` event.

Every turn is identified by a [`TurnId`](../api/ene-runtime.md). `run` returns that id (or `RunError::Busy` if a turn is already in flight). Turn-scoped chat events carry the same `turn` field. `Terminal` is emitted after conversation history commit and synchronous `finalize_turn` (affect persist); deferred LLM memory extraction and forgetting may still be in flight.

## Chat `EneEvent` variants

| `EneEvent` variant | Notes |
|---|---|
| `TurnStarted { turn, origin }` | After the provider stream opens |
| `TextDelta { turn, origin, delta }` | Plain-text chunks; emotion / performance markers are stripped. |
| `AudioChunk { turn, origin, pcm, sample_rate, is_final }` | Synthesized TTS audio streamed alongside `TextDelta` (only when a TTS provider is configured). See [Audio streaming](#audio-streaming). |
| `Performance { turn, origin, cues, source }` | Presentation cues from the Output Arbiter (`PerformanceCue` / `CueSource` in `ene-mind`). Replaces former `SpecialToken` + standalone `Expression` chat events. |
| `ToolCallStart` / `ToolCallResult` | Tool execution lifecycle (when the UI needs them). |
| `ToolBackgroundCompleted` | Deferred background tool finished (may arrive after `Terminal`). |
| `PermissionRequired` / `UserInputRequired` | Interactive tool gates. |
| `ContextCompressed { turn, origin, level }` | Thin signal that compression ran; details live on diagnostics. |
| `Terminal { turn, origin, reason }` | Exactly one per `Run`, after history commit and synchronous `finalize_turn` (affect persist). |
| `StatusChanged { status }` | Actor-level Idle / Running / Error. |

External JSON consumers should prefer `ene_runtime::PublicChatEvent` /
[`schemas/public-chat-event.v1.json`](../api/schemas/public-chat-event.v1.json).

### Audio streaming

When a TTS provider is configured (`ai.tts.provider != "none"`), the mind streaming pipeline feeds accumulated `TextDelta` text to the provider sentence-by-sentence and emits the synthesized audio as `AudioChunk` events interleaved with the text stream.

| Field | Type | Description |
|-------|------|-------------|
| `turn` | `TurnId` | The turn this audio belongs to |
| `origin` | `TurnOrigin` | Who initiated the turn |
| `pcm` | `Vec<f32>` | Interleaved mono PCM samples normalized to `[-1.0, 1.0]` |
| `sample_rate` | `u32` | Sample rate in Hz (e.g. `24000` for Kokoro ONNX) |
| `is_final` | `bool` | `true` on the terminal marker (empty `pcm`, `sample_rate = 0`) |

**Emission semantics:**

- Zero or more data chunks arrive with `is_final = false`, each carrying a slice of synthesized PCM. Chunks are roughly 0.25 s of audio at the provider's native sample rate.
- Exactly one terminal marker arrives with `is_final = true`, `pcm = []`, and `sample_rate = 0`. This signals that all sentences for the turn have been flushed.
- If TTS is disabled or the provider fails to initialize, no `AudioChunk` events are emitted — the text stream is unaffected.
- `AudioChunk` events may arrive after `Terminal` if synthesis of trailing sentences is still in flight; consumers should key on `is_final` rather than `Terminal` to know when audio playback can stop.

### Not on the chat bus

These are **not** chat `EneEvent` variants under API v1:

| Former / diagnostic | Where it lives now |
|---|---|
| `SpecialToken`, standalone `Expression` | Folded into `Performance` (or stripped from text) |
| `SessionSplit` | Compression / split via `diagnostics().manual_split()`; optional thin `ContextCompressed` |
| `PipelinePhase`, `PipelineMetrics`, `TaskProgress` | `handle.diagnostics().subscribe()` |
| `ToolHealth`, `ProviderHealth`, `ProviderFallback`, `MemoryWrite` | `handle.diagnostics().subscribe()` |
| `Lagged`, `ResyncNeeded` | Emitted when a broadcast subscriber overflows (#189); resync from snapshot |

`DiagnosticEvent::MemoryWrite` is emitted when deferred post-turn memory extraction fails. Failures are enqueued in `pending_memory_writes` (retry with backoff); `Terminal` is not delayed. Inspect with `/memory pending` / `/memory status`, or force a drain with `/memory retry`.

## Diagnostics

`handle.diagnostics()` returns a concrete `EneDiagnostics` facade. UIs do not implement a diagnostics trait. Use it for snapshots, tool inspection, manual compression/split, and the diagnostic event stream.

## App consumer checklist

Both `ene-cli` (`apps/ene-cli/src/stream.rs`) and `ene-desktop` (`apps/ene-desktop/src/ai_bridge.rs`) match on the minimal chat bus, including `Performance` and `Terminal`. When adding a new UI:

- Break the turn loop on `Terminal` with a matching `turn` — it is the only guaranteed one-per-run completion signal.
- Map `Performance` cues to VRM / CLI display; do not expect `SpecialToken` or standalone `Expression` on the chat bus.
- Treat `ContextCompressed` as an optional thin signal; poll `manual_split()` / diagnostics when you need compression details.
- When consuming `AudioChunk`, forward `pcm` to playback and viseme analysis; use `is_final` (not `Terminal`) to detect end-of-audio.

## Related documentation

- [`ene-runtime` API reference](../api/ene-runtime.md) — full `EneEvent` field reference and streaming dispatch
- [API v1 ADR](../architecture/api-v1.md) — locked host / event contracts
- [Session Split and Compression](session-split.md) — why compression is preferred over hard splits
- [Streaming Engine](streaming.md) — actor/handle architecture
- [Cognitive Runtime ADR](../architecture/cognitive-runtime.md)
