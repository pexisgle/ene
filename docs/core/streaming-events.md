# Streaming Events: Legacy vs Cognitive

`ene-core`'s actor dispatches every `EneCommand::Run` to one of two implementations of the streaming pipeline (see [`ene-core` API reference § Streaming Dispatch](../api/ene-core.md#streaming-dispatch)):

- **Legacy** (`run_stream_legacy`, in `streaming.rs`) — the original embed → recall → build-messages → stream loop.
- **Cognitive** (`streaming_cognitive::run_stream_cognitive`) — delegates prompt composition, recall, affect, and post-turn memory writing to `ene-cognition`'s `CognitionEngine`.

The dispatch condition is `cognition.enabled && memory.enabled && embedder.is_some()`; if cognition and memory are both enabled but no embedder is configured, the turn silently falls back to legacy. Both paths broadcast [`EneEvent`](../api/ene-core.md#eneevent) on the same channel, so **consumers do not need to know which path handled a given turn** — but the *set* of variants they can expect to see differs, which this page exists to pin down (see [API refactor plan](../architecture/api-refactor-plan.md), item 4).

## Variant-by-path matrix

| `EneEvent` variant | Legacy | Cognitive | Notes |
|---|---|---|---|
| `TextDelta` | ✅ | ✅ | Plain-text chunks from the LLM stream. |
| `SpecialToken` | ✅ | ✅ (conditionally) | Emitted whenever the raw model output still contains `<\|emo:name\|>` tokens. Under cognitive dispatch, in-stream tokens are only *suppressed* (not sent as `SpecialToken`) when `cognition.emotion.enabled && cognition.emotion.llm_expression_is_advisory` — i.e. the emotion engine is on and treats LLM proposals as advisory, so it prefers to resolve `Expression` itself instead of surfacing raw tokens. If `emotion.enabled == false`, or advisory mode is off, tokens stream through as `SpecialToken` exactly like the legacy path. |
| `Expression` | ❌ | ✅ (conditionally) | Engine-managed expression resolved by the cognitive runtime's Output Arbiter (#91) once a turn without pending tool calls completes. Only emitted when `cognition.emotion.enabled == true`; never emitted by the legacy path, which relies solely on inline `<\|emo:name\|>` tokens (`SpecialToken`). |
| `ToolCallStart` / `ToolCallResult` | ✅ | ✅ | Both paths call the same shared tool-execution machinery (`select_relevant_tools`, `perform_tool_executions`, `accumulate_tool_calls`, `finalize_tool_calls`), so tool-calling events are identical on both paths. |
| `PermissionRequired` / `UserInputRequired` | ✅ | ✅ | Also sourced from the shared `perform_tool_executions` — same reasoning as above. |
| `TaskProgress` | ✅ | ✅ | Forwarded from long-running tool calls on either path; not specific to either pipeline. |
| `PipelinePhase` | ✅ | ✅ | Marks entry into a pre-generation phase (`Embedding`, `Context Search`, `Prompt Building`). Both pipelines emit this, though the cognitive path's phases correspond to `CognitionEngine::before_turn`/`compose_prompt_packet` rather than the legacy recall/build-messages steps. |
| `PipelineMetrics` | ✅ | ❌ | Legacy-only today: emitted once, just before the first `TextDelta`, with per-phase elapsed milliseconds. The cognitive path does not currently emit an equivalent metrics snapshot — a gap worth closing if per-phase timing becomes important for the cognitive pipeline too. |
| `SessionSplit` | ✅ (actor-level) | ⚠️ (see below) | Not emitted by either streaming pipeline directly — it comes from the *actor's* auto-split check (`apply_pending_split`) or `EneHandle::manual_split()`, which run independently of which pipeline handled the preceding turn. |
| `Terminal` | ✅ | ✅ | Exactly one per `Run`, guaranteed by the shared `emit_terminal` + `terminal_emitted` guard on both paths. |
| `StatusChanged` | ✅ (actor-level) | ✅ (actor-level) | Emitted by the actor around dispatch, not by either streaming function itself. |

## The `SessionSplit` / compression gap

`EneHandle::manual_split()` and the actor's automatic split check both call into `handle_manual_split`, which branches on `cognition.enabled && cognition.context.compression_enabled`:

- **Legacy branch** (compression disabled, or cognition disabled): runs `ene_session::execute_split`, applies the split, and broadcasts `EneEvent::SessionSplit { summary, reason }`.
- **Compression branch** (`handle_manual_compression`): runs `execute_compression`, trims history, and returns a `SplitResult` to the caller — **but does not broadcast any `EneEvent`**. A consumer that only watches the event stream (rather than polling `manual_split()`'s return value) currently has no way to observe that a compression pass ran.

This is a known gap tracked by the [API refactor plan](../architecture/api-refactor-plan.md) item 4, "Phase B": the plan calls for emitting a compression-oriented event (or enriching `SessionSplit`) once this becomes a priority. Until then, `SessionSplit` should be read as "a **legacy-style hard split** occurred" — its absence does not mean nothing happened to the session's history.

## App consumer checklist

Both `ene-cli` (`apps/ene-cli/src/stream.rs`) and `ene-desktop` (`apps/ene-desktop/src/ai_bridge.rs`) already match on every current `EneEvent` variant, including `Terminal` and `Expression` — this was re-verified as part of the API refactor pass that produced this document. When adding a new UI, model your event loop on one of those two as a reference, and:

- Always break your loop on `Terminal`, not on any single "success" variant — it is the only guaranteed one-per-run signal.
- Handle `Expression` even if you also handle `SpecialToken`-derived emotion tokens; a character running with cognition and advisory emotion mode enabled will only ever send `Expression`, never emotion `SpecialToken`s.
- Treat `SessionSplit` as legacy-path-only signal per the gap above; don't build UX that assumes it fires on every context-management pass.

## Related documentation

- [`ene-core` API reference](../api/ene-core.md) — full `EneEvent` field reference and the streaming dispatch condition
- [Session Split and Compression](session-split.md) — why compression is preferred over hard splits
- [Streaming Engine](streaming.md) — actor/handle architecture (legacy-path-oriented; predates the cognitive dispatch and `Expression`/`Terminal` variants documented here)
- [Cognitive Runtime ADR](../architecture/cognitive-runtime.md)
- [API Refactor Plan](../architecture/api-refactor-plan.md) — item 4 (events/session migration)
