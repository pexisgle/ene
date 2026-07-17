# Session Management

`ConversationSession` is the runtime state holder for one active chat lifecycle. It is owned by `EneActor` and passed into streaming runs.

## What Session Owns

- Turn history used for prompt composition.
- Streaming display buffers and token carry.
- Memory context handles (`MemoryStore`, embedder, `session_id`).
- Character card state and current card metadata.

## Cognitive Runtime Behavior

The mind runtime uses session state together with `CognitionEngine`:

1. `before_turn` computes recall plan + affect updates.
2. `compose_prompt_packet` builds sectioned prompt context.
3. `after_turn` writes typed memory and persists affect.

This keeps one continuous session identity while context pressure is handled by compression instead of mandatory splitting.

## Session ID and Continuity

- `session_id` is generated at session creation and used for:
  - raw conversation logs
  - compression spans (`memory_spans`)
  - traceability/debugging in mind events
- Under mind compression mode, old turns are compacted while keeping the same session ID.

## CharacterCardV3 in Session

Session keeps the loaded `CharacterCardV3` so runtime can:

- Compile and inject Identity Kernel each turn.
- Load lorebook/style examples for recall and prompt sections.
- Resolve expression definitions for Output Arbiter.

## Related Docs

- `docs/reference/architecture/cognitive-runtime.md`
- `docs/reference/runtime/prompt.md`
- `docs/reference/runtime/session-split.md`
