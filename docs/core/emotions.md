# Emotion & Expression

Ene supports two expression paths: **legacy token mode** (when `cognition.emotion.enabled` is false or the legacy streaming pipeline is active) and **cognitive runtime mode** (engine-managed affect + Output Arbiter).

## Cognitive Runtime Path (default when `cognition.enabled`)

When `cognition.emotion.enabled` is true:

1. **Pre-turn:** `EmotionEngine` loads `AffectState` from the database, applies time-based decay and deterministic appraisal (gratitude, praise, insult, urgency, fatigue), and optionally merges an advisory LLM affect classifier proposal.
2. **Prompt:** `build_natural_dialogue_contract()` replaces the token-list PHI. The LLM is asked for natural dialogue only — no `<|emo:|>` tokens required.
3. **Post-stream:** `OutputArbiter` maps updated `AffectState` (+ optional LLM token hints) to a resolved expression with hysteresis.
4. **Event:** `EneEvent::Expression { name, source }` is emitted to consumers.
5. **Post-turn:** Updated `AffectState` (including `last_expression`) is persisted.

```
User input
  → EmotionEngine (decay + appraisal [+ optional classifier])
  → PromptPacket (Current Mood section + natural dialogue contract)
  → LLM stream (text only)
  → OutputArbiter
  → EneEvent::Expression
  → VRM / CLI display
```

### Configuration

See `cognition.emotion` in [settings.md](../configuration/settings.md):

| Key | Role |
|-----|------|
| `enabled` | Master switch for engine-managed emotion |
| `engine` | `deterministic`, `llm`, or `hybrid` (classifier) |
| `decay_half_life_minutes` | PAD decay toward neutral between turns |
| `expression_hysteresis_seconds` | Minimum hold time before expression changes |
| `llm_expression_is_advisory` | When true, stream tokens are accumulated for the arbiter instead of emitted immediately |
| `classifier_timeout_secs` / `classifier_min_confidence` | LLM affect classifier budget (#88) |
| `classifier_language` | Prompt library locale for classifier and natural-dialogue contract (`en` / `ja`) |

## Legacy Token Path

When emotion engine is disabled or the legacy pipeline runs, the LLM can produce `<|emo:name|>` special tokens to control the character's facial expression.

### Token Parsing

Implemented in `special_token.rs`:

| Function | Description |
|----------|-------------|
| `split_text_and_special_tokens(carry, chunk)` | Splits stream chunks into text fragments and `<\|...\|>` tokens. Tokens spanning chunk boundaries are held in `carry` |
| `extract_emotion_from_token(token)` | Extracts emotion name from `<\|emo:name\|>` (case-insensitive) |

### Legacy Data Flow

```
LLM stream → raw text chunks
  ↓
ene-core stream task: session.process_delta(chunk)
  ├── Text → EneEvent::TextDelta { delta }
  └── <|emo:name|> → EneEvent::SpecialToken { token }
       ↓
Consumer receives separate events:
  ├── CLI: TextDelta → print directly
  │       SpecialToken → extract_emotion_from_token → "[Emotion: name]"
  └── Desktop: TextDelta → EneStreamEvent::TextDelta
              SpecialToken → extract_emotion_from_token → EmoteToken
                → EmotionQueue → hold → fade-out → SetExpressions (VRM blendshapes)
```

**Important:** Emotion extraction from `TextDelta` happens inside `ene-core`'s stream task, not in the consumer. Consumers receive pre-parsed `SpecialToken` events.

### Emotion Expression Protocol (PHI)

`build_expression_phi()` injects an instruction block listing available `<|emo:name|>` tokens. Tokens are derived from `card.data.extensions["expressions"]`.

Default expressions (can be disabled per-card):

| Emotion | VRM Blendshape |
|---------|---------------|
| neutral | Default pose |
| happy | Defined value |
| sad | Defined value |
| angry | Defined value |
| relaxed | Defined value |
| surprised | Defined value |

Merged with `post_history_instructions` before injection.

## Per-Application Processing

| Application | Cognitive path | Legacy path |
|-------------|----------------|-------------|
| ene-desktop | `Expression` → `EmoteToken` → `EmotionPipelineState` | `SpecialToken` → `EmoteToken` |
| ene-cli | `Expression` → `[Expression: name]` | `SpecialToken` → `[Emotion: name]` |

Hold duration on desktop follows `cognition.emotion.expression_hysteresis_seconds` (default 4.0s).
