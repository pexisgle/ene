# `ene-mind` interface

## Role

The cognitive engine: prompt composition, recall, memory writing, emotion,
proactive speech, sessions, and commitments. The largest public surface in
the workspace, because `ene-runtime` (and tests) drive it through its
lifecycle DTOs.

## Public modules

| Module | Contents |
|---|---|
| `engine` | `CognitionEngine` facade (`new`, `before_turn`, `sync_character_memories`, …) |
| `lifecycle` | Turn DTOs: `TurnContext`, `PreTurnOutput`, `PostTurnInput`, `ComposedPrompt`, `HistoryEntry`, `PromptPacketMeta`, `interruption_note` |
| `character` | `CharacterProcessor`, identity-kernel compilation, lorebook injection |
| `session` | `ConversationSession`, `SessionId`, `CardName`, splitting (`SplitResult`, `TopicBoundaryTracker`), performance-marker parsing |
| `recall` | `RecallPlanner`, `RecallPlan`, `RecalledMemory`, `MemoryRecallCache`, hybrid runner, diversification |
| `memory_writer` | `MemoryWriter`, `MemoryArbiter`, decision/arbiter types, forgetting, reflection pipeline, tool grounding |
| `emotion` | `EmotionEngine`, `AffectProposal`, `TurnAffectInput` |
| `context` | `ContextManager`, `ContextBudget`, compression (`CompressionResult`, `CompressionLevel`, `execute_compression`, `pack_prompt`) |
| `prompt_packet` | `PromptPacket`, `PromptSection`, `PromptSectionKind` (16 section kinds with fixed render order) |
| `output` | `OutputArbiter`, `PerformanceCue`, `CueSource`, `PerfKind`, `MotionLayer`, expression decision types |
| `proactive` | proactive decision pipeline types (`ProactiveDecision`, `ProactiveObservation`, gates, quiet hours, world state) |
| `commitments` | `CommitmentLedger`, `CommitmentSyncContext` |
| `summarizer` | `summarize_conversation`, `ConversationSummaryResult` |
| `config` / `error` | `MindConfig` (and sub-configs), `CognitionError`, `MindError` |

## Key re-exports

- Memory types re-exported from `ene-core` for consumers (`Commitment`,
  `ActiveCommitmentPrompt`, …).
- Config section types (`MindConfig`, `SessionConfig`, `ProactiveConfig`,
  `QuietHoursConfig`, `MemoryApprovalConfig`, …).

## Dependencies

- Depends on: `ene-core`, `ene-config`, `ene-card`, `ene-ai`, `ene-rag`, `ene-util`.
- Used by: `ene-runtime`, `ene-cli`, `ene-desktop`.
- Explicitly **not** depended on (production): `ene-runtime`,
  `ene-plugin-host`, `ene-store` (dev-dependency only).

## Refactoring notes

- The `lifecycle` DTOs are the **runtime↔mind contract**: `ene-runtime`
  calls `before_turn` / prompt composition / finalize with these types.
  Change them with the runtime in mind.
- Mind reaches persistence **only** through `ene_core::MemoryPort` — never
  import `ene-store` types into cognitive modules. The
  `recall`/`memory_writer` modules are the ones most likely to tempt this;
  keep the seam.
- `PromptSectionKind::render_order()` is load-bearing: adding a section kind
  changes every prompt. Budget logic lives in `context`.
- The crate exposes many `pub` modules for streaming integration and tests;
  not all of them are intended for external consumers. When refactoring,
  prefer narrowing visibility to `pub(crate)` over deleting behaviour.
- Emotion (`PAD` affect) is persisted as `AffectState` + pending proposals;
  the store representation and the mind model must stay in sync through
  `ene-core` types only.
