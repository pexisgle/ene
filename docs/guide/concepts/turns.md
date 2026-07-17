# Turns and Streaming

A **turn** is one user message through LLM streaming (and optional tool calls) until the runtime emits a terminal chat event.

## Host API (mental model)

1. `EneHandle::open(config, card)` — ready handle; providers, store, tools, and mind are up before return
2. `run(input)` — starts a turn and returns a `TurnId`, or `Busy` if another turn is in flight
3. `subscribe()` — receive streaming chat events for that turn
4. `cancel(turn)` — cancel only the matching turn

Only one turn runs at a time (single-flight Busy).

## What you see as events

Typical flow: stream deltas → optional permission / user-input prompts → tool results → **Performance** cues for the avatar → **Terminal**.

Diagnostics (snapshots, tool lists, manual session split) go through `diagnostics()`, not the chat bus.

## Dig deeper

- [Streaming engine](../../reference/runtime/streaming.md)
- [Streaming events](../../reference/runtime/streaming-events.md)
- [API v2 ADR](../../reference/architecture/api-v2.md)
