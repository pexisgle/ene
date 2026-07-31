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
- **Why there is no `Scorer` trait (yet)**: an earlier revision introduced a `Scorer` trait to abstract "score one candidate against a context" across the memory and tool systems. It was removed during review (#302): nothing outside its own unit tests called it, and its tool implementor duplicated the inline logic in the selection pipeline. A trait that exists only to be implemented twice — one of them behind a feature flag — is a liability, not an extension point. When the document/workspace index (#185) introduces a genuine third scoring system, the abstraction can be reintroduced with a real consumer to shape it.
- **Scope note**: this crate is a *structural* separation. On top of that structure, the hybrid-score combination was redesigned from an additive weighted sum to a relevance-driven multiplicative form (#346), and the tool-selection score from an unnormalized field sum to a normalized, field-count-independent weighted average with a negative-example gate (#436). Both policies live here so the memory and tool sides cannot diverge again.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-rag --open
```

Start at the `scoring` and `decay` modules for the memory policy, and the `tool` module (feature-gated) for the tool-selection pipeline.

---

## Related
- [Memory System & Hybrid Recall](../concepts/memory-system.md)
- [Tool SDK Crates](tool-sdk.md)
- [System Architecture](../architecture.md)
