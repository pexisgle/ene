# ADR: Ene Cognitive Runtime Architecture

- **Status:** Accepted
- **Date:** 2026-06-28
- **Epic:** #63 — Redesign AI runtime as Ene Cognitive Runtime

## Context & Problem

The current Ene AI runtime bundles conversation history, long-term memory, emotions, and prompt construction into a relatively simple pipeline. This creates several issues for a long-running AI Companion / AITuber experience:

1. **Emotion control depends heavily on LLM `<|emo:name|>` tokens.** There is no engine-side persistent emotional state; if the LLM omits the token, the expression does not update.
2. **Memory is limited to `conversation_summaries` / `conversation_keyfacts`.** There is no support for memory kind, confidence, recency, emotional salience, or contradiction resolution.
3. **Session splits fragment memory and conversation continuity.** Summaries are created at split boundaries, and each split resets the session, losing the sense of an ongoing relationship.
4. **The prompt layer structure is weak.** Long contexts cause Character Drift — the LLM gradually forgets the character's core identity as the prompt fills with history.
5. **CCv3 lorebook / semantic settings are underutilized.** They are only included as inline text, not indexed for semantic retrieval.
6. **Forgetting is a hard delete for legacy keyfacts.** Typed memories support a faded / archived / superseded lifecycle (#76); legacy `conversation_keyfacts` migration is planned in #98.
7. **Codex-style explicit state (context packing, task ledger, tool result grounding) is not integrated** into the companion experience.

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
- Optional HyDE query expansion (`use_hyde` / `hyde_blend`) applied by `execute_hybrid_recall`

Downstream recall execution maps `MemoryStore::search` output to `RecalledMemory` values via `RecallResultMapper::map` or `RecallPlanner::explain_results`, attaching a primary `RecallReason` and score breakdown for debug, UX, and prompt introspection (#74).

When `mind.memory.rerank_enabled` is true, an optional LLM rerank stage (`MemoryRerankPipeline`) may reorder the top hybrid-search candidates before mapping. Disabled or failed rerank falls back to hybrid search order without changing `MemoryScoreBreakdown::total` (#77).

When `mind.memory.mmr_enabled` is true (default), a deterministic MMR diversification stage (`MemoryDiversifyPipeline`) runs after hybrid search and before optional reranking. It merges near-duplicate clusters, applies greedy MMR selection, enforces per-kind minimum slots, and rewards recall-source diversity. Hybrid scores are preserved unchanged (#78).

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

Deduplication uses normalized exact match first; optional pre-computed semantic matches (vector search) can collapse near-duplicates or trigger supersede/dispute logic. Until MemoryWriter orchestration (#100) wires embedding search, callers must populate `ArbiterContext::semantic_matches` themselves (e.g. from `MemoryJournal` scored search).

### Tool Result Grounding (#92)

Phase 8 grounds tool-call outcomes into typed memory with explicit safety constraints:

- `ene-runtime::streaming::perform_tool_executions` emits bounded `ToolResultSummary` entries for each call.
- `ene-mind::memory_writer::tool_grounding` sanitizes/truncates raw outputs (`max_summary_chars`) and masks screenshot payloads so large blobs are not persisted verbatim.
- When LLM extraction owns the turn, tool outcomes are judged in the **same** extractor call (as conversation context + soft hints). Routine successes are not auto-persisted.
- Deterministic `persist_success_procedure` / `persist_user_visible_episodic` default to `false`; `persist_failure_reflection` remains a fallback when LLM extraction does not own the turn.
- The cognitive streaming path forwards per-turn `tool_results` into `PostTurnInput`, enabling Memory Writer + Arbiter persistence with `source_ref` prefix `tool:` when a candidate is kept.

### Companion Commitment Ledger

Promises, tasks, and follow-ups (e.g. “let’s discuss this next time”) are tracked in a dedicated `commitments` table, separate from generic typed memory recall scoring.

| Concept | Location |
|---------|----------|
| Domain types (`Commitment`, `CommitmentStatus`) | `ene-store` |
| Persistence (`insert_commitment`, `list_active_commitments`, …) | `ene-store::MemoryStore` |
| Sync from arbiter results | `ene-mind::commitments::CommitmentLedger` |

**Relationship to `MemoryKind::Commitment`:** Extractors produce `MemoryCandidate { kind: Commitment, commitment_due }`. The Memory Arbiter persists these as typed memories. `CommitmentLedger::sync_from_applied_decisions` (or `arbitrate_apply_and_sync`) then creates an active ledger row linked via `source_memory_id`.

**Lifecycle:** `active` → `done` | `cancelled` | `stale`. Overdue rows with a parsed `due_at` can be transitioned to `stale` via `mark_stale_commitments`.

**Prompt injection:** Active commitments are returned by `list_active_commitments` / `CommitmentLedger::active_prompt_candidates` **without** vector similarity — they are always candidates for the Active Commitments section of `PromptPacket` (#87).

### Context Compression
Rolling compression that summarizes old conversation turns into compact memory spans. Unlike session splits, compression preserves continuity — the session ID remains the same, and the sense of an ongoing conversation is maintained.

## Consequences & Migration Strategy

### Positive
- **Character Drift reduction** — Identity Kernel is always present, never truncated
- **Memory continuity** — No session split breaking; compression preserves context
- **Rich semantic memory** — CCv3 lorebook becomes a searchable memory index
- **Sophisticated recall** — Multi-factor scoring (vector + recency + salience + confidence + affect + commitments)
- **Persistent emotion** — Engine-managed affect state, not dependent on LLM tokens
- **User agency** — Memory inspect / pin / archive / forget / dispute UX
- **Natural forgetting** — Faded / archived / superseded lifecycle instead of hard delete

### Migration Path
- **Phase 0–10** are implemented — `ene-mind` is the sole streaming implementation in `ene-runtime::streaming.rs`
- **#98 (migration policy)**:
  - Legacy summary/keyfact tables are **read-only** (no new summaries/keyfacts)
  - Unmigrated legacy summaries/keyfacts are not merged into normal mind recall; use `/memory migrate legacy` explicitly
  - Optional one-shot CLI migration maps summaries → `Episodic`, keyfacts → `UserProfile`/`Preference`, logs → `memory_spans` (transactional)
  - After migration, recall uses typed memory only; legacy rows remain for audit unless reset
  - `mind.memory.require_migration` blocks recall when summaries/keyfacts remain unmigrated (logs alone do not block)
  - Affect **persistence** runs each turn; affect **computation** is implemented by `EmotionEngine` (#86) with optional LLM classifier (#88) and `OutputArbiter` expression resolution (#89, #91)
- **#80** replaces automatic session splits with rolling context compression triggers

## References

- Epic: #63 — Redesign AI runtime as Ene Cognitive Runtime
- Full Phase & Dependency Map: `#63` issue body
- Current architecture: `docs/reference/architecture/overview.md`
- Memory system: `docs/reference/memory/memory.md`
- Prompt construction: `docs/reference/runtime/prompt.md`
- Session splitting: `docs/reference/runtime/session-split.md`
- Emotion handling: `docs/reference/runtime/emotions.md`
