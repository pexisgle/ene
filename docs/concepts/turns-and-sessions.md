# Turns & sessions

## Turns

A **turn** is one unit of conversation: a user message (or a proactive /
scheduled trigger) plus everything the system does in response.

### Turn identity and concurrency

- Every turn has a `TurnId`.
- `EneHandle::run` returns immediately with the `TurnId`; streaming events
  arrive on the event bus.
- Turn execution is **single-flight**: if another turn is still running,
  `run` fails with `RunError::Busy`. `cancel(&turn_id)` aborts the active
  turn (`CancelError::TurnMismatch` if the id is stale).

### Turn origins

| Origin | Trigger |
|---|---|
| `user` | A message you send |
| `proactive` | The companion decides to speak on its own (proactive pipeline) |
| `scheduled` | A persistent schedule fired (see [Schedules](../guides/schedules.md)) |

### What happens inside a turn

1. `before_turn` — affect decay, recall planning + hybrid search,
   character/lorebook sync, prefetch of prompt data.
2. Prompt packet composition — sectioned system prompt + budgeted history.
3. LLM streaming — text deltas, tool calls, performance cues.
4. Mid-turn tools — permission-gated execution, user-input prompts when a
   tool asks a question.
5. Finalize — affect proposal, session state update.
6. History commit + `Terminal` event; background memory extraction follows.

The full pipeline is described in
[Cognitive runtime](../reference/architecture/cognitive-runtime.md).

## Events

The runtime exposes **three separate channels** so traffic classes cannot
starve each other:

| Channel | Contents | Consumption |
|---|---|---|
| Chat bus | `EneEvent`: turn lifecycle, text deltas, tool calls/results, permission requests, performance cues, beat pulse | broadcast, via `subscribe()` |
| Lifecycle bus | `LifecycleEvent`: status changes, pending-candidate notifications, background-tool completion | broadcast, via `subscribe_lifecycle()` |
| Audio stream | `AudioChunk` (TTS audio for playback) | single consumer, via `take_audio_stream()` |

All chat/lifecycle events have stable JSON mirrors (`PublicChatEvent`,
`PublicLifecycleEvent`) for external clients — see
[API v1](../reference/architecture/api-v1.md).

## Sessions

A **session** is a contiguous conversation with one character, identified
by a `SessionId`. Sessions exist so history, memory provenance, and
summaries stay organized.

- A session ends when it is explicitly split, when the idle timeout
  (`mind.session.session_timeout_minutes`) elapses, or when the topic
  boundary detector decides the conversation changed enough.
- When a session ends, the conversation is **summarized** and the summary
  becomes part of the character's context for future sessions.
- Sessions can be listed, exported (versioned, redacted JSON), imported,
  searched, archived, and unarchived — via the CLI `/session` command or
  the API v1 session methods.

## Context compression

When history would overflow the model's context window, the runtime
compresses it: older messages are summarized into a rolling "active scene
summary" while recent messages stay verbatim. The event bus emits
`context_compressed` so clients can show what happened. Compression is
triggered proactively (with a configurable wait) so a turn does not stall
mid-stream waiting for it.

## Undo

The actor keeps an undo stack for state-changing operations (permission
grants, memory edits, schedule changes). `EneHandle::undo` /
`/undo` reverts the most recent operation and reports what was undone.
