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
  sequence for a single turn (fields omitted for brevity — run
  `cargo doc -p ene-runtime --open` and see `handle::EneEvent` for the
  authoritative variant list and fields):

  ```text
  EneEvent::TurnStarted { turn, origin }
    │
    ├── EneEvent::TextDelta { turn, origin, delta }     (LLM streaming text)
    ├── EneEvent::Performance { turn, origin, cues, source } (Avatar expression / motion)
    ├── EneEvent::ToolCallStart { turn, origin, name, arguments }
    ├── EneEvent::ToolCallResult { turn, origin, name, result }
    ├── EneEvent::PermissionRequired { turn, origin, request_id, .. }
    ├── EneEvent::UserInputRequired { turn, origin, request_id, prompt }
    ├── EneEvent::ContextCompressed { turn, origin, level }
    │
    └── EneEvent::Terminal { turn, origin, reason }     (Turn complete, session committed)
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

### Recovering from a broadcast lag

The `broadcast` chat and lifecycle buses are lossy: when a subscriber falls
behind, `recv` returns `RecvError::Lagged(n)` and the `n` skipped events are
gone for good — possibly including the in-flight turn's `Terminal`. Gaps are
never silent (a `DiagnosticEvent::Lagged` is emitted too), and the recovery
procedure is uniform across consumers:

- **Chat-bus lag** — the streamed view of the in-flight turn is no longer
  trustworthy. Query `EneHandle::active_turn()` (a lightweight, mailbox-free
  read of the single-flight gate); when it returns `Some(turn)`, cancel that
  turn with `EneHandle::cancel` so the actor emits a fresh `Terminal` and
  releases the gate — otherwise the next `run` fails with `RunError::Busy`.
  Then tear down any local per-turn UI state.
- **Lifecycle-bus lag** — there is no turn to cancel (lifecycle notifications
  are turn-independent). Simply re-derive the state the missed notification
  would have carried, e.g. re-query `EneHandle::candidates()` for the pending
  candidate count.

Run `cargo doc -p ene-runtime --open` and see `EneHandle::active_turn` for the
authoritative procedure.

---

## 2. Session Lifecycle & Session Splitting

A **session** is a contiguous dialogue history stored in SQLite (`ene-store`).

- **Active Session**: A turn always appends user input and assistant responses to the current `SessionId`.
- **Automatic Compression & Splitting**: When token count exceeds configured limits (`mind.context.max_tokens`), `SessionManager` performs context compression and creates a new session branch.
- **Summary Fact Generation**: Before splitting a session, key facts and dialogue summaries are extracted and stored into `MemoryStore` as permanent episodic memory.

### Topic-Boundary Detection (#367)

Independent of token pressure, `ene-mind` detects *topic* boundaries so later
stages can decide when to compress (#368) or split (#369) on semantic grounds.
A naive "cosine similarity between two consecutive utterances" is not used —
it fails on backchannels ("うん" is dissimilar to its neighbours yet on-topic),
on short utterances with unstable embeddings, and on topics that drift
gradually so no single low-similarity pair ever appears.

Instead the detector maintains a **topic centroid**: an exponentially weighted
moving average of the embeddings that belong to the current topic. Each
completed turn is scored with a composite of three signals:

| Signal | Contribution |
|---|---|
| Cosine distance from the centroid | Primary indicator of a topic shift |
| Silence since the previous utterance | A long pause raises boundary likelihood |
| Turn count of the current topic | An over-long topic likely holds several topics |

The turn-count term is the accumulator that makes **gradual** drift detectable:
a centroid that tracks its topic closely keeps the distance term small, but an
over-long topic accumulates turn-count pressure that pushes an already
suspicious topic (drift or silence) across the threshold. It is a *soft* cap:
`weight_topic_length` must stay below `boundary_threshold`, so the turn-count
term can never fire a boundary on its own — a perfectly coherent, long-running
topic is not force-split. Utterances shorter
than `mind.topic_boundary.min_utterance_chars` are treated as backchannels —
they neither score a boundary nor update the centroid, so a "うん" cannot
corrupt the topic model. No hard-coded keyword list ("ところで", "話は変わるけど",
…) is used; the embeddings carry the signal, which keeps the detector working
across languages.

Detection runs after the response text has streamed (so it never delays the
user-facing reply) and before the deferred memory-writing slot spawns, and it
considers the user input embedding of the completed turn. The centroid
lifecycle is:

| Trigger | Centroid |
|---|---|
| Compression (#368) | **Not reset** — compression is physical, not a topic boundary |
| Session split (#369) | Reset |
| Confirmed boundary | Restart as the new topic's centroid |

The detector only produces a signal and a composite score (surfaced as a
`SplitReason::Composite`); acting on it belongs to #368 and #369. All
thresholds and weights are configurable under `mind.topic_boundary`.

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

### Prompt Library & Language Packs

The user-facing LLM instruction strings (system framing, emotion rules,
summarizer and extractor prompts, split reasons, and so on) are not hard-coded.
`ene-config`'s `PromptLibrary` loads them at runtime from a per-language JSON
pack at `assets/lang/{lang}/prompts.json`, selected by the `mind.emotion.classifier_language`
setting. This keeps prompt text editable without recompiling, and adding a
language is a matter of dropping in a new `assets/lang/{lang}/` directory — no
Rust code change to the loading path is required.

When a runtime pack is missing or unreadable (unit tests, CI, or a stripped
install), `PromptLibrary` falls back to a compile-time embedded pack. Only the
languages listed in `ene_config::SUPPORTED_LANGUAGES` (currently `en` and `ja`)
carry an embedded fallback; any other language falls back to English. The
embedded packs are generated from the same `crates/ene-config/prompts/` sources
as the shipped assets, and a unit test asserts the two stay byte-for-byte
identical.

---

## 4. Affect & Emotion Model (PAD)

Ene models character emotional state using a PAD-derived (**Pleasure-Arousal-Dominance**) space, represented by `ene-core::AffectState`:

- **Valence ($\in [-1, 1]$)**: Positive vs negative feeling (the "pleasure" axis).
- **Arousal ($\in [-1, 1]$)**: Excited vs calm energy level.
- **Dominance ($\in [-1, 1]$)**: Dominant/confident vs submissive state.
- Plus trust, affinity, irritation, curiosity, and fatigue dimensions that extend beyond the classic 3-axis PAD model — see `ene_core::AffectState` (`cargo doc -p ene-core --open`) for the full field list.

### Emotional Dynamics
- **Natural Decay**: Affect values drift toward baseline over time.
- **Classification**: Text responses and user input trigger subtle affect shifts via `ene-mind`'s `EmotionEngine`.
- **Performance Cues**: `ene-mind`'s output arbitration maps affect state to `PerformanceCue`s (expression/motion) sent as `EneEvent::Performance` for `ene-desktop`/VRM avatar playback.
