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
run_ai_with_tools → TextDelta(String)
  ↓
Consumer (ai_bridge / CLI) receives
  ↓
session.process_delta(chunk) splits
  ├── Text → display
  └── <|emo:name|> → emotion token processing
       ↓
GUI: EmotionQueue → SetExpressions (VRM blendshapes)
CLI: "[Emotion: name]" in magenta text
```

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
| ene-desktop (GUI) | `TextDelta` → `process_delta()` → `EmotionQueue` → `process_emotion_queue` (4s hold, then fade-out) → `SetExpressions` |
| ene-cli (CLI) | `TextDelta` → `process_delta()` → `[Emotion: name]` in magenta |
