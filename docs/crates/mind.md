# `ene-mind`

> **Crate**: `ene-mind` | **Role**: Pure cognitive turn engine — session, recall, prompt composition, affect, performance

`ene-mind` is the pure cognitive core of Ene: session state, memory extraction, recall planning, the affect/emotion engine, context budgeting, proactive speech evaluation, output/performance arbitration, and prompt packet composition all live here. `ene-runtime` only *invokes* `CognitionEngine`; it does not reimplement mind logic in its own streaming path.

---

## Architectural boundaries

- `ene-mind` **never** depends on `ene-runtime` or `ene-plugin-host` — this prevents circular dependencies between the host facade and the cognitive layer it drives.
- `ene-mind`'s cognitive-logic modules (recall, memory arbiter, forgetting, character sync, journal, self-reflection) call the persistence layer only through the `ene_core::MemoryPort` trait — never the concrete `ene_store::MemoryStore` directly — so they can be unit-tested against an in-memory test double without SQLite.
- `ene-mind` calls `ene-store` only through its public API; it never issues raw SQL or `sea-orm` queries. `ene-store` remains the sole SQLite owner.
- Session state absorbed from the former standalone session crate lives under the `session` module.

## Design rationale

- **Why `MemoryPort` instead of a concrete store type**: it decouples cognitive logic (recall planning, the memory arbiter, forgetting lifecycle) from any specific persistence backend, so those code paths can run against an in-memory double in tests without paying for SQLite setup, and so `ene-store` can evolve its schema without every mind-side call site needing to change.
- **Why affect/emotion state is richer than a plain 3-axis PAD model**: `ene-core::AffectState` extends pleasure/arousal/dominance (as `valence`/`arousal`/`dominance`) with trust, affinity, irritation, curiosity, and fatigue dimensions — these feed proactive-speech gating and performance-cue selection, not just the presentation layer.
- **Why prompt composition is a distinct, budget-aware step** rather than a flat message array: sections (identity, affect, recalled memories, tool specs, dialogue history) compete for a fixed token budget, and truncation order matters for keeping identity/safety rules intact under pressure — see `docs/concepts/turn-and-session.md` §3 for the section layout.

## API reference

Struct and method signatures are not duplicated here — they drift. Generate rustdoc for the authoritative, current API:

```sh
cargo doc -p ene-mind --open
```

Start at `CognitionEngine` (`engine` module) for the turn-facing entry points, and `AffectState` / `EmotionEngine` (re-exported from `ene-core` / `emotion`) for affect.

---

## Related
- [Turns & Sessions](../concepts/turn-and-session.md)
- [Memory & Recall](../concepts/memory-system.md)
