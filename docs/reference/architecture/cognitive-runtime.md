# ADR: Ene Cognitive Runtime Architecture

- **Status:** Accepted
- **Date:** 2026-06-28

## Context

Ene's cognitive runtime treats the LLM as an utterance generator over explicitly managed state: identity kernel, typed memory, affect, performance cues, context budgeting, and commitment ledger. `ene-mind` owns the turn pipeline; `ene-runtime` integrates it into streaming and events; `ene-store` persists typed memory and the commitment ledger.

## Decision

**Adopt the Ene Cognitive Runtime architecture.** Treat the LLM not as the entity that implicitly holds personality and memory, but as an **engine that generates natural utterances from an explicit cognitive state managed by Ene**.

Ene will explicitly manage:
- Identity Kernel
- Typed Memory (with lifecycle)
- Semantic Character Memory
- Context Compression
- Recall Planning
- Affect / Mood / Relationship State
- Expression Arbitration
- Memory Writing
- Context Budget Management
- Companion Task Ledger

## Crate Boundaries & Responsibilities

| Component | Crate | Responsibility |
|---|---|---|
| Identity Kernel | `ene-mind::character` | Compile CCv3 into an immutable character identity block always present in the prompt |
| Typed Memory Store | `ene-store` | CRUD + hybrid search for typed memories (kind, confidence, recency, salience, vector) |
| Memory Extraction (Deterministic) | `ene-mind::memory_writer` | Explicit remember/forget safety net + LLM-failure fallback (soft signals are LLM-only) |
| Memory Extraction (LLM) | `ene-mind::memory_writer` | Primary path: importance judgment and kind selection into `MemoryCandidate` items |
| Memory Arbiter | `ene-mind::memory_writer` | Validate candidates against existing memories, compute confidence, deduplicate, resolve contradictions |
| Recall Planner | `ene-mind::recall` | Generate a `RecallPlan` with search intent and budget hints for downstream recall execution |
| Hybrid Search Scoring | `ene-store` | Score memories by vector similarity + recency + salience + confidence + affect + commitments |
| Emotion Engine | `ene-mind::emotion` | Deterministic affect computation from conversation dynamics + optional LLM classifier |
| Expression Arbiter | `ene-mind::output` | Map `AffectState` to character expressions with hysteresis and configured constraints |
| Context Budget Manager | `ene-mind::context` | Allocate token budgets across `PromptPacket` sections |
| Context Compression | `ene-mind::context` | Rolling compression of old conversation turns into memory spans |
| PromptPacket Composer | `ene-mind::prompt_packet` | Assemble sectioned prompt packets with independent budget per section |
| Companion Commitment Ledger | `ene-mind::commitments` | Track promises, tasks, and follow-ups the companion has made |
| Conversation History | `ene-mind` | Maintain turn history; session splits are phased out in favor of compression triggers |
| Streaming Integration | `ene-runtime` | Orchestrate the full turn lifecycle and emit events |

### Dependency Rules

- `ene-mind` **depends on** `ene-store`, `ene-config`, `ene-ai`
- `ene-mind` **does NOT depend on** `ene-runtime` or `ene-tool-host` (prevents circular dependencies)
- `ene-runtime` depends on `ene-mind` and integrates the mind runtime into `ene-runtime::streaming.rs`; missing store/embedder prerequisites fail closed with a typed error
- `ene-store` remains the exclusive owner of `sea-orm` SQLite operations — extraction, arbitration, and recall planning logic lives in `ene-mind`, not `ene-store`
- `ene-store` **does NOT depend on** `ene-ai` or `ene-mind`
- `ene-vrm` **does NOT depend on** `ene-mind` or `ene-runtime`

## Turn Lifecycle

```mermaid
sequenceDiagram
    participant User
    participant Streaming as ene-runtime (streaming)
    participant PreTurn as Pre-turn Analyzer
    participant Recall as Recall Planner
    participant Emotion as Emotion Engine
    participant Composer as Context Composer
    participant LLM
    participant Arbiter as Output Arbiter
    participant Writer as Memory Writer
    participant Store as Cognitive Memory Store

    User->>Streaming: user input
    Streaming->>PreTurn: pre_turn.analyze(input, history, affect)
    PreTurn->>Recall: trigger recall planning
    Recall-->>Composer: recall plan (queries, kind hints, budget)
    Note over Recall,Store: Downstream recall execution runs hybrid search using the plan
    Composer->>Store: hybrid search (kind, recency, salience, vector)
    Store-->>Composer: recalled memories + commitments
    PreTurn->>Emotion: update affect from turn dynamics
    Emotion-->>Composer: affect state
    Composer->>Composer: build PromptPacket<br/>(Identity Kernel + Recall + Affect + History + Tools)
    Composer->>LLM: prompt packet
    LLM-->>Arbiter: raw response + optional expression hints
    Arbiter->>Arbiter: validate expression, apply hysteresis
    Arbiter-->>Streaming: text + expression events
    Streaming-->>User: display output
    Streaming->>Writer: post-turn write(input, response, affect)
    Writer->>Writer: extract candidates (LLM-first; remember/forget safety net)
    Writer->>Store: arbiter validates → write typed memories
    Writer->>Store: execute forgetting lifecycle
    Writer->>Emotion: persist affect state changes
```

### Lifecycle Steps

1. **Pre-turn Analysis** — Assess the user input, current affect state, and recent history to determine the turn's intent, emotional tone, and memory retrieval needs.
2. **Recall Planning** — Generate a `RecallPlan` with search queries, memory kind filters, and token budget hints. Downstream recall execution uses the plan to run hybrid search against the typed memory store.
3. **Emotion Update** — Compute the new `AffectState` from turn dynamics (user sentiment, topic valence, relationship cues). Apply decay to previous affect.
4. **Context Composition** — Build a `PromptPacket` with sectioned layers: Identity Kernel → Recalled Memories → Commitments → Affect State → Scene → Style Examples → History → Current Input.
5. **LLM Generation** — Send the `PromptPacket` to the LLM provider. The LLM may optionally provide expression hints.
6. **Output Arbitration** — Validate and map affect+response to character expressions. Apply hysteresis to prevent expression flickering.
7. **Post-turn Writing** — Run the LLM extractor as the primary path. Deterministic matchers cover only explicit remember/forget (remember is a hint when LLM succeeds; forget always reaches the arbiter as a safety net). On LLM failure, empty result, or when disabled, remember patterns and configured tool-grounding fallbacks apply. The Memory Arbiter validates against existing memories, computes confidence, and writes to the store.
8. **Forgetting Lifecycle** — Age existing memories according to decay curves via `ForgettingLifecycle::apply`. Transition through `active → faded → archived` statuses. User explicit forget (`user_deleted`) and contradiction paths (`disputed`, `superseded`) remain in the Memory Arbiter.

## Key Terminology

### Identity Kernel
The immutable character identity block compiled from CCv3 character card data by `ene-mind::character::CharacterCompiler` (#82). Always placed at the top of every prompt packet. Contains structured header lines (name, role, core personality, speech style, hard instruction) plus optional sections from `system_prompt`, `description`, `scenario`, and `creator_notes`. CBS macros (`{{char}}`, `{{user}}`, …) are expanded at compile time. **Core header lines must never be truncated**; optional sections respect `mind.character.identity_kernel_max_tokens`.

### CCv3 Semantic Memory (#83)
`character_book` entries compile into character-scoped typed memories (`MemoryKind::Semantic`, `MemorySource::Ccv3`) with stable `source_ref` values under `ccv3:lorebook:*`. Constant entries are pinned; key-triggered entries include `Triggers: …` at the start of the stored **content** (not the title). `CognitionEngine::sync_character_memories` reindexes on card change: removed entries are archived; changed content for the same `source_ref` is **superseded** and re-embedded.

### Style Example Retrieval (#84)
`mes_example` dialogue chunks compile to `ccv3:style:*` procedure memories and are selected per turn by deterministic intent heuristics (greeting, comforting, joking, etc.). Selected examples appear in the `## Style Examples` prompt section (budget: `style_example_budget_tokens`) and may be dropped on overflow without affecting the Identity Kernel.

### Typed Memory
A memory with an explicit `MemoryKind`:
- **Episodic** — Specific events / conversations
- **Semantic** — Facts and knowledge
- **Procedural** — How-to knowledge and user preferences for actions
- **Preference** — User likes, dislikes, and traits
- **Relationship** — Information about the user-companion relationship
- **Commitment** — Promises, tasks, and follow-ups

### MemoryStatus
The lifecycle state of a memory:
- `active` — Currently relevant and retrievable
- `faded` — Decayed but still retrievable with lower priority
- `archived` — No longer shown in normal recall but preserved
- `superseded` — Replaced by a newer, conflicting memory

### AffectState
Persistent emotional state with dimensions:
- **Valence** (pleasure — displeasure)
- **Arousal** (excitement — calm)
- **Dominance** (control — submission)
- **Discrete emotions** (joy, sadness, anger, fear, surprise, neutral, etc.) with per-emotion intensity

### PromptPacket
A sectioned prompt structure where each section has its own token budget, managed by the Context Budget Manager:
1. Identity Kernel (always first, never truncated)
2. Style Examples (from CCv3 `mes_example`)
3. Recalled Memories
4. Active Commitments
5. Current Affect State
6. Conversation History (most recent N turns)
7. Expression PHI (`build_expression_phi` — emotion protocol + card post-history instructions)
8. Current User Input

> **Known limitation:** CCv3 lorebook `selective`, `secondary_keys`, and `position` fields are not yet interpreted by the cognitive runtime.

### RecallPlan
A query plan generated by the Recall Planner that specifies:
- Search queries (natural language + embedding)
- Memory kind filters (hints for downstream recall execution)
- Token budget allocated for recalled content
- Hybrid search hints such as vector similarity threshold, minimum total score, recency half-life, and optional query affect

Downstream recall execution maps `MemoryStore::search` output to `RecalledMemory` values via `RecallResultMapper::map` or `RecallPlanner::explain_results`, attaching a primary `RecallReason` and score breakdown for debug, UX, and prompt introspection (#74).

A deterministic MMR diversification stage (`MemoryDiversifyPipeline`) runs after hybrid search. It merges near-duplicate clusters, applies greedy MMR selection, enforces per-kind minimum slots, and rewards recall-source diversity. Hybrid scores are preserved unchanged (#78).

### Expression Arbiter
Receives the current `AffectState`, optional LLM expression hints, and character expression definitions. Outputs a resolved expression with:
- **Hysteresis** — prevents rapid expression changes (configured in seconds)
- **Advisory mode** — LLM hints are treated as suggestions, not commands, when configured

### Memory Arbiter
Sits between memory extractors and the typed memory store in `ene-mind::memory_writer::arbiter`. For each `MemoryCandidate` it emits a traceable decision:

| Decision | When |
|----------|------|
| `Persist` | Candidate passes validation and has no conflicts |
| `Ignore` | Low confidence, invalid fields, exact/semantic duplicate, or deletion target not found |
| `Supersede` | New evidence replaces an existing memory (transactional insert + mark old `superseded`) |
| `MarkDisputed` | Weak contradiction — existing memory flagged for user review |
| `MarkUserDeleted` | User deletion request matched an existing memory |
| `AskConfirmationLater` | Ambiguous contradiction deferred until user confirmation |

Validation gates:
- `min_confidence_to_persist` from `MindMemoryConfig` (default `0.65`)
- Non-empty title/content
- `source_quote` must appear in the turn text (procedure memories from tool results are exempt when `source_quote` is empty)
- Deletion candidates require `deletion_target_key`

Deduplication uses normalized exact match first; optional pre-computed semantic matches (vector search) can collapse near-duplicates or trigger supersede/dispute logic.

### Tool Result Grounding

Tool-call outcomes are grounded into typed memory with explicit safety constraints:

- `ene-runtime::streaming::perform_tool_executions` emits bounded `ToolResultSummary` entries for each call.
- `ene-mind::memory_writer::tool_grounding` sanitizes/truncates raw outputs (`max_summary_chars`) and masks screenshot payloads so large blobs are not persisted verbatim.
- When LLM extraction owns the turn, tool outcomes are judged in the **same** extractor call (as conversation context + soft hints). Routine successes are not auto-persisted.
- Deterministic tool grounding persists successful calls as `Procedure`, failed calls as `Reflection`, and short user-visible successes as `Episodic` when appropriate.
- The cognitive streaming path forwards per-turn `tool_results` into `PostTurnInput`, enabling Memory Writer + Arbiter persistence with `source_ref` prefix `tool:` when a candidate is kept.

### Companion Commitment Ledger

Promises, tasks, and follow-ups (e.g. “let’s discuss this next time”) are tracked in a dedicated `commitments` table, separate from generic typed memory recall scoring.

| Concept | Location |
|---------|----------|
| Domain types (`Commitment`, `CommitmentStatus`) | `ene-store` |
| Persistence (`insert_commitment`, `list_active_commitments`, …) | `ene-store::MemoryStore` |
| Sync from arbiter results | `ene-mind::commitments::CommitmentLedger` |

**Relationship to `MemoryKind::Commitment`:** Extractors produce `MemoryCandidate { kind: Commitment, commitment_due }`. `CommitmentLedger::apply_commitment_candidates` (or `arbitrate_apply_and_sync`) writes active ledger rows ledger-first. Optional typed `MemoryKind::Commitment` rows may reference the ledger via `typed_memories.commitment_id`.

**Lifecycle:** `active` → `done` | `cancelled` | `stale`. Overdue rows with a parsed `due_at` can be transitioned to `stale` via `mark_stale_commitments`.

**Prompt injection:** Active commitments are returned by `list_active_commitments` / `CommitmentLedger::active_prompt_candidates` **without** vector similarity — they are always candidates for the Active Commitments section of `PromptPacket`.

### Context Compression

Rolling compression summarizes old conversation turns into compact memory spans. The session ID stays the same so conversation continuity is preserved.

## References

- [Architecture overview](overview.md)
- [Memory system](../memory/memory.md)
- [Prompt construction](../runtime/prompt.md)
- [Session split](../runtime/session-split.md)
- [Emotion handling](../runtime/emotions.md)
