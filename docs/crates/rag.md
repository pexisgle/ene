# `ene-rag`

> **Crate**: `ene-rag` | **Role**: RAG policy layer — memory recall scoring/decay and retrieval-augmented tool selection

`ene-rag` consolidates the retrieval-augmented-generation *policy* that used to be scattered across the persistence crate (`ene-store`'s pure scoring/decay functions) and a standalone tool-selection crate (the former `ene-tool-rag`). It owns scoring and ranking, half-life decay, embedding-cache management, and post-processing (rerank, per-category limits). It is a peer of `ene-voice` in the crate map.

---

## Architectural boundaries

- The scoring/decay core depends on `ene-core` **only** (plus generic deps). Because the `ene-rag → ene-store` edge does not exist, a store↔rag dependency cycle is impossible at compile time — this is a design goal, not an incidental property.
- Persistence is reached through the `ene_core::EmbeddingStorePort` and `ene_core::MemoryPort` traits, never the concrete `ene_store::MemoryStore`. `ene-store` implements those ports; `ene-rag` programs against the abstraction.
- The tool-selection pipeline (the `tool` module) is behind the `tool` Cargo feature because it needs embedding/LLM machinery (`ene-ai`). Persistence (`ene-store`) and cognitive (`ene-mind`) callers use the default feature set — pure scoring/decay only — so the embedding stack never leaks into the persistence layer (AGENTS.md: `ene-store` ↛ `ene-ai`). Only `ene-runtime` enables `tool`.
- Pure state-machine lifecycle validators (`validate_transition` / `validate_user_restore`) are **not** here — they are lifecycle policy and stay in `ene-store`. Only scoring/decay policy moved.

## Design rationale

- **Why a dedicated policy crate**: `ene-store`'s mandate is persistence (SQLite/SeaORM, schema, vector primitives, candidate gathering). Pure scoring and decay functions touch no database, so housing them there violated the "store is only about persistence" principle and made the store depend on policy it should not own.
- **Why `MemoryPort::search` returns gathered candidates, not scored results**: the store's job ends at gathering candidates from its indexes; scoring is policy. Returning unranked `GatheredCandidate`s lets callers compose `store.search(...)` with `ene_rag::score_and_rank`, keeping the gather/score split explicit at the trait boundary.
- **Why one half-life decay primitive**: recall recency (`recency_score`) and lifecycle retention (`decay_score`) independently implemented the same `exp(-λ·age)` formula with different anchors. A single `half_life_decay` kernel removes the duplication while preserving each caller's anchor and post-processing exactly.
- **Why a `Scorer` trait rather than a dynamic registry**: the memory side scores a gathered candidate into a score breakdown while the tool side scores a field embedding into a scalar — different candidate and score types. A string-keyed registry would erase those types; the trait lets each RAG system keep its own candidate/context/score types while sharing the surrounding pipeline. This is also the extension point for the planned document/workspace index (#185).
- **Scope note**: this crate is a *structural* separation. Scoring formulas are preserved exactly as they were; the hybrid-score additive-structure redesign is tracked separately (#346 / #436).

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-rag --open
```

Start at the `scoring` and `decay` modules for the memory policy, the `Scorer` trait for the extension point, and the `tool` module (feature-gated) for the tool-selection pipeline.

---

## Related
- [Memory System & Hybrid Recall](../concepts/memory-system.md)
- [Tool SDK Crates](tool-sdk.md)
- [System Architecture](../architecture.md)
