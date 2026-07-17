# Sessions

A **session** is the active conversation context for a character: history, card-driven identity, and when the thread should split or compress.

## What shapes a session

- Character card (`CharacterCardV3`) and macros (CBS-style expansion)
- Recent messages kept in the working context
- Mind policies for compression and splitting (timeouts, topic change, manual split)

## Splitting

Long or drifting chats can start a new session while preserving continuity via memory and summaries. Triggers include idle timeout, topic-change detection, and explicit/manual split from diagnostics.

## Dig deeper

- [Session management](../../reference/runtime/session.md)
- [Session splitting](../../reference/runtime/session-split.md)
- [Prompt construction](../../reference/runtime/prompt.md)
