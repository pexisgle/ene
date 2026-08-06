# `ene-core` interface

## Role

Persistence-agnostic domain vocabulary and the **port traits** that decouple
the cognitive layer from the persistence layer. This crate is the lowest
common denominator: it depends on nothing internal to the workspace.

## Public modules

All modules are private; the public surface is the re-exports below (plus
the port module).

## Key public types

| Area | Items |
|---|---|
| Affect | `AffectState` (PAD: valence/arousal/dominance, trust, affinity, irritation, curiosity, fatigue), `DiscreteEmotion`, `PendingAffectProposal` |
| Commitments | `Commitment`, `NewCommitment`, `CommitmentStatus`, `ActiveCommitmentPrompt` |
| Key facts | `KeyFact` |
| Typed memory | `MemoryItem`, `NewMemoryItem`, `MemoryKind` (10 kinds), `MemoryStatus`, `MemoryScope`, `MemorySource`, `MemoryConfidence`, `MemorySalience`, `MemoryEdit`, `MemorySearchOptions`, `HybridSearchWeights`, `ScoredMemory`, `MemoryScoreBreakdown`, `MemoryOutcome`, `AffectAnnotation`, `ContradictionKeyMatch`, `ForgettingPolicy`, `Query`, `TimeRange`, `GatheredCandidate`, `MemoryCandidateSource`, `MemoryJournalListOptions` |
| Pending candidates | `PendingCandidate`, `PendingCandidateStatus`, `PendingCandidateEdit`, `NaturalDecayReport` |
| Pending writes | `PendingMemoryWrite`, `PendingMemoryWriteStatus` |
| Schedules | `Schedule`, `NewSchedule`, `ScheduleKind`, `ScheduleAction`, `ScheduleRun`, `ScheduleRunStatus`, `ScheduleConfirmation`, `ScheduleError`, `first_run_at`, `next_occurrence_after` |
| Spans | `NewMemorySpan`, `ActiveSceneSummaryRow` |
| Workspace | `NewWorkspaceChunk`, `WorkspaceChunkHit`, `WorkspaceFileRow`, `WorkspaceIndexStatus`, `WorkspaceSearchQuery` |
| Ports | `MemoryPort`, `EmbeddingStorePort`, `ToolFailureSignalPort`, `WorkspaceDocumentPort` (+ error types) |

## The port traits (the refactoring seams)

| Trait | Contract | Implemented by |
|---|---|---|
| `MemoryPort` | Typed-memory CRUD, affect state, commitments, pending candidates, recall search | `ene_store::MemoryStore`; test doubles in `ene-mind` tests |
| `EmbeddingStorePort` | Vector/embedding persistence for memory and tool RAG | `ene_store::MemoryStore` |
| `WorkspaceDocumentPort` | Workspace document index CRUD | `ene_store::MemoryStore` |
| `ToolFailureSignalPort` | Tool failure signals for RAG negative-example gating | `ene_store` |

## Dependencies

- Depends on: nothing internal (serde, chrono, thiserror, tracing, schemars, async-trait).
- Used by: `ene-store`, `ene-mind`, `ene-rag`, `ene-runtime`.

## Refactoring notes

- **Adding** a domain type here is low-risk (everyone can already see it).
- **Changing** an existing type or port method is high-risk: it ripples into
  the store's SeaORM conversions, the mind's cognitive logic, and the
  runtime's streaming path. Prefer additive fields with defaults.
- `MemoryKind::WorldState` is reserved and unreachable today — producers
  deliberately reject it; do not add producers without a design.
- The port traits are the place to look when decoupling work is planned:
  any new persistence capability that mind needs should be declared here,
  not imported from `ene-store`.
