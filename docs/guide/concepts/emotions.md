# Emotions and Avatar Performance

The model can express mood; desktop plays it back on the VRM avatar.

## Two layers

1. **Affect** — engine-side emotional state, updated across turns and persisted when store is enabled
2. **Performance cues** — final presentation hints (`EneEvent::Performance`) that the desktop maps to blendshapes / animations

Mind owns cue arbitration. `ene-vrm` only renders; it does not import mind types.

## Tokens in text

Emotion markers such as `<|emo:name|>` may appear in model output; the runtime parses them as part of the performance path rather than as separate ad-hoc UI events.

## Dig deeper

- [Emotion and Performance](../../reference/runtime/emotions.md)
- [Avatar Performance ADR](../../reference/architecture/avatar-performance.md)
- [Desktop app](../apps/desktop.md)
