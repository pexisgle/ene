# Cognitive runtime

`ene-mind` is the cognitive engine. It treats the LLM as an utterance
generator: personality, memory, emotions, and behaviour are explicit state
the engine manages. This page documents the pipeline in detail.

## Components

`CognitionEngine` composes:

| Component | Responsibility |
|---|---|
| `ContextManager` | Token budget for the context window, rolling compression, scene summaries |
| `RecallPlanner` | Turn intent → search plan (scopes, kinds, budget) |
| `MemoryRecallCache` | L1 in-memory recall cache (L2 is `ene-store`) |
| `MemoryWriter` | Post-turn extraction (deterministic + LLM) and arbitration |
| `EmotionEngine` | PAD affect: decay, deterministic updates, optional LLM classifier |
| `CharacterProcessor` | Identity kernel compilation, lorebook indexing/injection |
| `PromptPacket` | Sectioned prompt composition |
| `OutputArbiter` | Expression/motion validation, hysteresis, cue selection |
| `CommitmentLedger` | Promise/task tracking and prompt injection |

The engine never touches the database directly: it programs against
`ene_core::MemoryPort` (and `EmbeddingStorePort` for vectors), so it is
unit-testable without SQLite.

## Turn pipeline

### 1. `before_turn`

- Load the character's `AffectState`, decay it to the current time
  (valence/arousal/dominance, trust, affinity, irritation, curiosity,
  fatigue), and apply any pending classifier proposal from the previous
  turn.
- Plan recall from the new message: detect intent (question about the
  user, the world, a past event, …), choose scopes/kinds and a budget.
- Execute hybrid recall: vector similarity (when an embedder exists),
  lexical overlap, recency, emotional match, relationship score,
  contradiction/stale penalties; diversify (MMR); bump access counts.
- Prefetch what the prompt needs: identity kernel (with time-of-day /
  scene / relationship-stage lines), guaranteed lorebook entries, scene
  summary, active commitments, workspace chunks.

### 2. Prompt composition

The prompt is assembled as **sections** in a fixed render order:

```text
Platform contract
Lorebook (before char)
Identity kernel          ← character definition, budgeted to 1/8 of the
                           context window (clamped 400..4000 tokens)
Lorebook (after char)
Character state          ← affect/mood summary
Scene state              ← rolling summary
Semantic context         ← lorebook + character memories
User profile             ← user/relationship/preference memories
Workspace context        ← retrieved document chunks
Active commitments
Episodic memories
Style examples
Interruption note        ← only after an interrupted response
Output contract          ← expression PHI, NG phrases
User input               ← the current turn
```

Sections are packed against the model's effective context window
(provider-advertised window minus response reserve and safety margin).
When history would overflow, older messages compress into the scene
summary (see [Turns & sessions](../../concepts/turns-and-sessions.md#context-compression)).

### 3. Streaming and tools

Tokens stream from the provider; mid-turn tool calls execute through the
runtime with permission gating, and interactive tools can pause the turn to
ask the user a question. Performance cues (expression/motion/lookat) are
emitted as the model or affect engine requests them.

### 4. `after_turn` / finalize

- The emotion engine produces an **affect proposal** (deterministic update
  plus, when enabled, an LLM classifier result). The proposal is stored
  pending and merged at the start of the next turn, so the current
  response is not re-rendered mid-stream.
- Conversation history is committed to the store; the `Terminal` event
  closes the turn.
- **Deferred**: memory extraction + arbitration, forgetting pass, and
  self-reflection run in the background after the terminal event, so the
  user never waits for memory work.

## Emotion model

Affect is **PAD** (pleasure–arousal–dominance) plus relationship metrics:

- `valence` (-1..1), `arousal` (-1..1), `dominance` (-1..1)
- `trust`, `affinity` (-1..1) — relationship to the user
- `irritation`, `curiosity`, `fatigue` (0..1)
- `mood_label`, `last_expression`, discrete emotion intensities

Values decay toward the card's `affect_baseline` (all zeros when absent)
over time. `AffectState` is per character (and optionally per user).

## Memory writer & arbiter

Post-turn, `MemoryWriter` runs:

1. Deterministic extraction (commitments, user-stated facts, tool
   grounding).
2. LLM extraction when a classifier model is configured.
3. `MemoryArbiter` decisions: semantic duplicate detection (embedding
   similarity with normalized-title fallback), contradiction handling
   (new memory supersedes old), confidence/salience scoring, and
   approval-queue deferral when `mind.memory_approval.require_approval`.

Outcomes are reported as `MemoryWriteOutcome` (success or retry/permanent
failure with a pending queue id).

## Proactive speech

The proactive pipeline decides whether the companion speaks on its own:

- **Deterministic gates** — cooldown, minimum idle time, quiet hours
  (with per-day schedule and suppression policy), fatigue suppression,
  paused flag.
- **Sources** — conversation, activity, screen summary, memory, window
  title (configurable level).
- **Decision LLM** — when gates pass, a lightweight model call chooses
  whether to speak and what (with urgency scoring and a
  `SILENT_TOKEN` option).
- **World state** — optional snapshots of the environment enable trend
  detection ("you've been idle for an hour").
- **Pending confirmations** — commitments awaiting confirmation can be
  re-surfaced after a minimum age.

Proactive turns run with `TurnOrigin::Proactive` and can be disabled
entirely (`mind.proactive.enabled = false`).

## Output arbitration

`OutputArbiter` validates every performance cue before it reaches the
avatar: expressions must exist on the model, motions must exist in the
catalog, and hysteresis/rate limits prevent flicker. Cue sources are
`affect` (emotion engine) or `llm` (explicit model request); source
priority decides conflicts.

## Summarizer

At session boundaries, `summarize_conversation` produces a summary that
becomes part of the character's ongoing context (`SceneState`), so
long-running companions do not forget the arc of a conversation.

## Boundaries

- `ene-mind` does not depend on `ene-runtime` or `ene-plugin-host`.
- It calls persistence only through `ene_core::MemoryPort`.
- `ene-store` is a dev-dependency only (integration tests).
