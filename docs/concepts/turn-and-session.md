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

### Retroactive Topic Compression (#368)

A topic change is hard to judge instantly and is not worth delaying the
response for, so compression is applied **retroactively** after the turn
completes rather than before the response:

1. Turn N arrives → the response is produced immediately using the history
   as-is; nothing waits on boundary detection.
2. After the turn completes — in the same deferred slot as memory writing and
   affect classification — #367's detector scores the completed turn, now with
   both the user input and the assistant response available as judgment
   material.
3. If a boundary is detected, the span *before* the boundary (the previous
   topic) is summarized into a single scene span.
4. Turn N+1's history is "previous-topic summary" (injected as the
   `## Current Scene` section) plus "turn N onward".

The result is history that looks as if the topic had already been switched at
the boundary — applied one turn late, but imperceptible in practice. Running
detection and summarization behind the response keeps latency unaffected, and
seeing the assistant response makes it easier to tell a temporary digression
from a genuine topic change. The centroid is **not** reset by compression
(compression is a physical operation, not a topic boundary); only a session
split (#369) resets it.

Two compression triggers now coexist:

| Trigger | Condition | Unit |
|---|---|---|
| **Topic boundary (primary)** | The retroactive compression above | One topic = one span |
| **Window pressure (secondary)** | Retained history reaches the token ceiling | Oldest span |

Because boundaries compress naturally, window-pressure compression is a safety
net for a topic that runs too long without a detected boundary. Its trigger is
now **token-based** — an estimate of the retained history's tokens against a
configurable ceiling (`ContextConfig::context_pressure_tokens`) — replacing the
former message-count ratio heuristic and its hard-coded `1.25` factor. The
existing `CompressionLevel` hierarchy (Scene → Chapter → Arc) is preserved: one
topic maps to one Scene summary, and chapter/arc rollups aggregate multiple
scenes as before.

---

## 3. Prompt Packet Composition

Rather than sending raw chat arrays to the LLM, `PromptComposer` builds structured `PromptPacket`s and packs them against the model's context window:

```
┌──────────────────────────────────────────────────────────┐
│ Protected System Identity (Character card & core rules)  │  (Required — never dropped)
├──────────────────────────────────────────────────────────┤
│ Current PAD Affect State & Presentation Cues             │
├──────────────────────────────────────────────────────────┤
│ Recalled Memories (Hybrid vector + lexical facts)        │  (Droppable by priority)
├──────────────────────────────────────────────────────────┤
│ Tool Capabilities & IPC Specifications                   │
├──────────────────────────────────────────────────────────┤
│ Recent Session Dialogue History                           │  (Trimmed oldest-first)
└──────────────────────────────────────────────────────────┘
```

Packing is a **priority-ordered fill** against the model's effective context
window (#364, #370): the available window is `effective_window − response_reserve
− safety_margin`, and `pack_prompt` keeps the required sections (platform
contract, identity kernel, output contract, user input) unconditionally, then
fills the remaining capacity by section priority, dropping the lowest-priority
droppable sections first when the prompt overflows. There are no per-section
sub-budgets — each section's *size* is bounded by its content producer (recall
result limit, lorebook token budget, identity-kernel cap), so packing only
decides *which* sections survive. As a last resort the oldest dialogue messages
are trimmed.

The identity kernel's `Speech style` line is the character's own declaration:
it renders `extensions.ene.speech` (length, first/second person, politeness,
verbal tics) when the card defines it, and a concise language-localized default
otherwise. It is never guessed from prompt text, so a Japanese card cannot get
an English keyword match or unrelated instructions as its speech style; labels
and the default follow the app language (`mind.language`), while the defined
values keep the card author's language.

The kernel also renders the optional roleplay blocks from `extensions.ene`
when they are defined and the per-turn state matches:

- `relationship_stages` — a `Relationship tone:` line from the stage with the
  highest `threshold` not exceeding the current user-relationship affinity
  (the persistent `affect.affinity` dimension, −1.0..=1.0). Cards define
  stages like `{ "threshold": 0.3, "label": "close friend", "tone": "speak
  with easy warmth" }`; no matching stage renders no line.
- `time_periods` — a `Time-of-day behavior:` line when the local hour falls
  in the defined period (`morning` 05–10, `afternoon` 11–16, `evening`
  17–20, `night` 21–04).
- `scene_behaviors` — a `Scene behavior:` line when any keyword matches the
  active scene text (the rolling-compression scene summary, falling back to
  the card's `scenario`).

All three blocks are optional and per-turn: a card without them compiles the
same kernel as before, and a defined block whose condition does not hold
simply renders nothing.

`extensions.ene.ng_expressions` is injected into the output contract (the
PHI section that always survives packing) as a localized `Never say:` list,
so the character's prohibitions reach the model on every chat turn. Cards
without the list keep the output contract exactly as the host built it.

`extensions.ene.style_examples` (labeled response examples) replace the flat
`mes_example` chunking as the style-example source when present: a label
equal to a selector intent tag (`greeting`, `comforting`, `joking`,
`serious_explanation`, `refusal`, `tool_use`) selects through the intent
pipeline, any other label is matched as a case-insensitive substring of the
user's input, and no match falls back to the first examples — the same
fallback the flat examples use.

### Prompt Library & Language Packs

The user-facing LLM instruction strings (system framing, emotion rules,
summarizer and extractor prompts, split reasons, and so on) are not hard-coded.
`ene-config`'s `PromptLibrary` loads them at runtime from a per-language JSON
pack at `assets/lang/{lang}/prompts.json`, selected by the app-wide
`mind.language` setting (per-task overrides:
`mind.emotion.classifier_language` for classifier/cognitive prompts,
`mind.context.compression_language` for the summarizer — empty inherits
`mind.language`). This keeps prompt text editable without recompiling, and
adding a language is a matter of dropping in a new `assets/lang/{lang}/`
directory — no Rust code change to the loading path is required.

When a runtime pack is missing or unreadable (unit tests, CI, or a stripped
install), `PromptLibrary` falls back to a compile-time embedded pack. Only the
languages listed in `ene_config::SUPPORTED_LANGUAGES` (currently `en` and `ja`)
carry an embedded fallback; any other language falls back to English. The
embedded packs are generated from the same `crates/ene-config/prompts/` sources
as the shipped assets, and a unit test asserts the two stay byte-for-byte
identical.

Deterministic memory heuristics use the same pack layout: `ene-config`'s
`PatternLibrary` loads per-language data from `assets/lang/{lang}/patterns.json`,
selected by `mind.language` (memory extraction is deliberately independent of
the classifier setting). The pack holds
the forget-detection regexes *and* the recall-intent keyword lists (episodic /
preference / relationship / affective / procedure hints). Adding a
language is therefore a data-only change (drop in a `patterns.json`), and a
language without a pack falls back to English patterns. Explicit *remember*
requests are owned by the LLM extractor and are no longer pattern-matched;
*forget* requests keep their deterministic safety net and are always applied
even when the LLM owns the turn.

---

## 4. Affect & Emotion Model (PAD)

Ene models character emotional state using a PAD-derived (**Pleasure-Arousal-Dominance**) space, represented by `ene-core::AffectState`:

- **Valence ($\in [-1, 1]$)**: Positive vs negative feeling (the "pleasure" axis).
- **Arousal ($\in [-1, 1]$)**: Excited vs calm energy level.
- **Dominance ($\in [-1, 1]$)**: Dominant/confident vs submissive state.
- Plus trust, affinity, irritation, curiosity, and fatigue dimensions that extend beyond the classic 3-axis PAD model — see `ene_core::AffectState` (`cargo doc -p ene-core --open`) for the full field list.

### Emotional Dynamics
- **Natural Decay**: Affect values drift toward baseline over time. The baseline is per-character and defined in the card under `extensions.ene.affect_baseline` (all eight dimensions; decay currently applies to the five decaying dimensions — valence, arousal, dominance, irritation, fatigue); cards without it decay toward all zeros, which is the legacy behaviour.
- **Pre-utterance affect in the prompt**: At turn start, `EmotionEngine` applies decay, optional long-conversation fatigue, and any pending classifier proposal from the *previous* turn. The `CharacterState` section documents that this mood does **not** yet reflect the current user utterance — immediate reaction is left to the chat model’s reply.
- **Post-turn classification**: After the assistant responds, the affect classifier sees both the user utterance and the reply, then parks a proposal for the next turn. There is no deterministic keyword appraisal layer.
- **Performance Cues**: End-of-turn expression comes from a single
  `resolve_expression` decision (LLM proposal canonical, affect as fallback).
  Motion / look-at markers still accumulate mid-turn via `PerformanceArbiter`.
  The result is sent as `EneEvent::Performance` for `ene-desktop`/VRM playback.
