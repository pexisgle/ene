# Emotion Tokens

The LLM can produce `<|emo:name|>` special tokens to control the character's facial expression.

## Token Parsing

Implemented in `special_token.rs`:

| Function | Description |
|----------|-------------|
| `split_text_and_special_tokens(carry, chunk)` | Splits stream chunks into text fragments and `<\|...\|>` tokens. Tokens spanning chunk boundaries are held in `carry` |
| `extract_emotion_from_token(token)` | Extracts emotion name from `<\|emo:name\|>` (case-insensitive) |

## Data Flow

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
              SpecialToken → extract_emotion_from_token → EneStreamEvent::SpecialToken
                → EmotionQueue → SetExpressions (VRM blendshapes)
```

**Important:** Emotion extraction from `TextDelta` happens inside `ene-core`'s stream task, not in the consumer. Consumers receive pre-parsed `SpecialToken` events. There is no need to call `extract_emotion_from_token` on `TextDelta`.

## Emotion Expression Protocol

`build_expression_phi()` injects an instruction block into the prompt, listing available `<|emo:name|>` tokens. Tokens are derived from `card.data.extensions["expressions"]`.

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

| Application | Handling |
|-------------|----------|
| ene-desktop (GUI) | `SpecialToken` → `extract_emotion_from_token` → `EneStreamEvent::SpecialToken` → `EmotionQueue` → 4s hold → fade-out → `SetExpressions` |
| ene-cli (CLI) | `SpecialToken` → `extract_emotion_from_token` → `[Emotion: name]` in magenta |
