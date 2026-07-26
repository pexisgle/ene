# Turns, Sessions, & Cognitive Processing

This document explains the core conversation turn model, session lifecycle, prompt packet composition, affect/emotions (PAD model), and streaming event mechanics.

---

## 1. Conversation Turn Model

A **turn** represents a single complete interaction exchange between a host application and Ene.

### Single-Flight Turn Execution
- Host calls `EneHandle::run(message)` to initiate a turn.
- A unique `TurnId` (UUID) is assigned to the turn.
- The turn runs as a single-flight execution shell. If another turn is attempted while one is active, `RunError::Busy` is returned.
- To stop an ongoing LLM generation, the host invokes `EneHandle::cancel(turn_id)`.

### Event Bus Architecture
`ene-runtime` splits its event traffic across three dedicated channels by
traffic class, so a burst on one channel can never starve or lag consumers
of another:

- **Chat bus** (`EneEvent`, via `EneHandle::subscribe`) — a `broadcast`
  channel (capacity 1024) carrying lightweight, ordered, turn-scoped chat
  events. Multiple subscribers allowed. Below is the chat bus's event
  sequence for a single turn (variant names in this diagram are
  illustrative — see [`ene-runtime`'s crate docs](../crates/runtime.md)
  for the exact current variant set):

  ```text
  EneEvent::TurnStarted { turn_id }
    │
    ├── EneEvent::TokenStream { chunk }        (LLM streaming tokens)
    ├── EneEvent::Performance { cue }           (Avatar facial expression / motion)
    ├── EneEvent::ToolCallStarted { tool_name } (Tool invocation indicator)
    ├── EneEvent::ToolCallFinished { tool_name }
    │
    └── EneEvent::Terminal { turn_id, status } (Turn complete, session committed)
  ```

- **Audio channel** (`AudioChunk`, via `EneHandle::take_audio_stream`) — a
  bounded `mpsc` channel carrying synthesized TTS PCM. Single-consumer:
  ownership of the receiver transfers on the first call and every later
  call returns `None`. Kept off the chat bus because PCM payloads are
  heavyweight relative to chat events and would otherwise inflate every
  chat subscriber's `broadcast` buffer.
- **Lifecycle bus** (`LifecycleEvent`, via `EneHandle::subscribe_lifecycle`)
  — a small-capacity `broadcast` channel for turn-independent notifications
  (`StatusChanged`, `PendingCandidateAvailable`, `ToolBackgroundCompleted`).
  Multiple subscribers allowed, same as the chat bus.

---

## 2. Session Lifecycle & Session Splitting

A **session** is a contiguous dialogue history stored in SQLite (`ene-store`).

- **Active Session**: A turn always appends user input and assistant responses to the current `SessionId`.
- **Automatic Compression & Splitting**: When token count exceeds configured limits (`mind.context.max_tokens`), `SessionManager` performs context compression and creates a new session branch.
- **Summary Fact Generation**: Before splitting a session, key facts and dialogue summaries are extracted and stored into `MemoryStore` as permanent episodic memory.

---

## 3. Prompt Packet Composition

Rather than sending raw chat arrays to the LLM, `PromptComposer` builds structured `PromptPacket`s with strict budget boundaries:

```
┌──────────────────────────────────────────────────────────┐
│ Protected System Identity (Character card & core rules)  │  (High Priority)
├──────────────────────────────────────────────────────────┤
│ Current PAD Affect State & Presentation Cues             │
├──────────────────────────────────────────────────────────┤
│ Recalled Memories (Hybrid vector + lexical facts)        │  (Budget Constrained)
├──────────────────────────────────────────────────────────┤
│ Tool Capabilities & IPC Specifications                   │
├──────────────────────────────────────────────────────────┤
│ Recent Session Dialogue History                           │  (Truncated as needed)
└──────────────────────────────────────────────────────────┘
```

Budget allocation dynamically truncates oldest dialogue messages while maintaining identity and safety rules under token pressure.

---

## 4. Affect & Emotion Model (PAD)

Ene models character emotional state using the 3D **Pleasure-Arousal-Dominance (PAD)** space:

- **Pleasure ($P \in [-1, 1]$)**: Positive vs negative valence.
- **Arousal ($A \in [-1, 1]$)**: Excited vs calm energy level.
- **Dominance ($D \in [-1, 1]$)**: Dominant/confident vs submissive state.

### Emotional Dynamics
- **Natural Decay**: Affect values drift toward baseline over time.
- **Classification**: Text responses and user input trigger subtle affect shifts via `PadClassifier`.
- **Performance Cues**: `PadEmotion` maps to `PerformanceCue` expressions (e.g., Happy, Angry, Surprised, Thinking) sent to `ene-desktop` for VRM avatar playback.
