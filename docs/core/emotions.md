# Emotion & Performance

Ene supports two presentation mechanisms within the mind stream: **token compatibility mode** (when `mind.emotion.enabled` is false) and **engine-managed affect** (Output Arbiter → `Performance` cues).

Under API v2, chat consumers receive [`EneEvent::Performance`](streaming-events.md) — not standalone `SpecialToken` or `Expression` events.

## Mind Runtime Path

When `mind.emotion.enabled` is true:

1. **Pre-turn:** `EmotionEngine` loads `AffectState` from the database, applies time-based decay and deterministic appraisal (gratitude, praise, insult, urgency, fatigue), then consumes any pending post-turn classifier proposal from the previous turn.
2. **Prompt:** `build_natural_dialogue_contract()` replaces the token-list PHI. The LLM is asked for natural dialogue only — no `<|emo:|>` tokens required.
3. **Post-stream:** `OutputArbiter` maps updated `AffectState` (+ optional LLM token hints) to resolved presentation cues with hysteresis.
4. **Event:** `EneEvent::Performance { turn, cues, source }` is emitted to consumers.
5. **Post-turn:** Updated `AffectState` (including last expression / cue state) is persisted before `Terminal`.

```
User input
  → EmotionEngine (decay + appraisal [+ pending classifier from previous turn])
  → PromptPacket (Current Mood section + natural dialogue contract)
  → LLM stream (text only)
  → OutputArbiter
  → EneEvent::Performance
  → VRM / CLI display
```

`PerformanceCue` / `CueSource` are owned by `ene-mind` (runtime re-exports). Desktop maps cue names to VRM playback without importing mind into `ene-vrm`.

### Configuration

See `mind.emotion` in [settings.md](../configuration/settings.md):

| Key | Role |
|-----|------|
| `enabled` | Master switch for engine-managed emotion |
| `engine` | `deterministic`, `llm`, or `hybrid` (classifier) — see [Engine modes](#engine-modes) |
| `decay_half_life_minutes` | PAD decay toward neutral between turns |
| `expression_hysteresis_seconds` | Minimum hold time before expression / cue changes |
| `llm_expression_is_advisory` | When true, stream tokens are accumulated for the arbiter instead of driving cues immediately |
| `classifier_timeout_secs` / `classifier_min_confidence` | Post-turn async classifier budget and merge gate (#88) |
| `classifier_language` | Prompt library locale for classifier and natural-dialogue contract (`en` / `ja`) |
| `classifier_model` | Chat model for the classifier (default `google/gemini-2.5-flash-lite` on OpenRouter) |
| `classifier_max_tokens` | Max completion tokens for classifier calls (`0` = no cap) |

### Post-turn async classifier

When `engine` is `llm` or `hybrid`, Ene runs the affect classifier **after** the assistant response is produced.

- Input: turn-start `AffectState` snapshot + recent conversation history (including the current `user + assistant` exchange)
- Output: absolute post-conversation estimates for `valence`, `arousal`, `irritation`, and `affinity`
- Success: proposal is stored as pending with `source_turn_id = N` (completed user turn) and blended once at the **next** pre-turn when `current_user_turn == N + 1` (weighted by `confidence`)
- Failure/timeout: logged and ignored (deterministic path remains active)
- Stale/future pending proposals are dropped (too old or tagged for the current/future turn)

At **INFO** level you should see:
- `Starting post-turn affect classifier` when the async job starts
- `Post-turn affect classifier estimate complete` with the full estimate when classification succeeds
- `Blended post-turn classifier estimate into affect` at the **next** turn start when the pending proposal is merged

If you see no classifier logs, check that `mind.emotion.engine` is `hybrid` or `llm` (not `deterministic`).

### Engine modes

| Mode | Pre-turn rules | Post-turn classifier |
|------|----------------|----------------------|
| `deterministic` | Yes (gratitude, insult, decay, …) | **No** |
| `hybrid` (default) | Yes | Yes — estimate blended next turn |
| `llm` | **No** (decay only) | Yes — estimate blended next turn |

Use `hybrid` unless you explicitly want to disable either the rule-based path or the classifier.

The classifier uses a **dedicated provider instance** (not the main stream client), strict JSON Schema output (`response_format` with `strict: true`), optional `classifier_max_tokens`, and resilient transport fallbacks for OpenRouter compatibility.

## Token Compatibility Path

When the emotion engine is disabled, the LLM may still produce `<|emo:name|>` special tokens. The stream task strips markers from `TextDelta` and the Output / performance path may surface them as `Performance` cues rather than separate chat events.

### Token Parsing

Implemented in mind special-token helpers:

| Function | Description |
|----------|-------------|
| `split_text_and_special_tokens(carry, chunk)` | Splits stream chunks into text fragments and `<\|...\|>` tokens. Tokens spanning chunk boundaries are held in `carry` |
| `extract_emotion_from_token(token)` | Extracts emotion name from `<\|emo:name\|>` (case-insensitive) |

### Token Data Flow

```
LLM stream → raw text chunks
  ↓
ene-runtime / mind stream path
  ├── Text → EneEvent::TextDelta { turn, delta }
  └── <|emo:name|> → stripped from text; may become Performance cues
       ↓
Consumer:
  ├── CLI: TextDelta → print; Performance → "[Performance: name]" (or similar)
  └── Desktop: TextDelta → AI text; Performance → PerformanceCue / EmoteToken → VRM
```

### Emotion Expression Protocol (PHI)

When emotion is disabled, `build_expression_phi()` may inject an instruction block listing available `<|emo:name|>` tokens derived from `card.data.extensions["expressions"]`.

Default expressions (can be disabled per-card):

| Emotion | VRM Blendshape |
|---------|---------------|
| neutral | Default pose |
| happy | Defined value |
| sad | Defined value |
| angry | Defined value |
| relaxed | Defined value |
| surprised | Defined value |

Merged with `post_history_instructions` before injection when that path is active.

## Per-Application Processing

| Application | Chat event | Downstream |
|-------------|------------|------------|
| ene-desktop | `Performance` | `AppEvent::PerformanceCue` → `EmoteToken` → VRM blendshapes |
| ene-cli | `Performance` | Printed / logged cue names |

Hold duration on desktop follows `mind.emotion.expression_hysteresis_seconds` (default 4.0s).

## Related Documents

- [Avatar Performance ADR](../architecture/avatar-performance.md) — `PerformanceCue` markers, arbiter, and VRM LayerComposer
- [Streaming Events](streaming-events.md) — `EneEvent::Performance`
