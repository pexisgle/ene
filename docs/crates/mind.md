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
- **Why affect/emotion state is richer than a plain 3-axis PAD model**: `ene-core::AffectState` extends pleasure/arousal/dominance (as `valence`/`arousal`/`dominance`) with trust, affinity, irritation, curiosity, and fatigue dimensions — these feed proactive-speech gating and performance-cue selection, not just the presentation layer. All eight dimensions (plus the derived mood label) reach the proactive decision prompt; fatigue additionally backs a deterministic pre-LLM gate (`mind.proactive.fatigue_suppression_threshold`), so a tired character stays silent even if the decision model would otherwise speak.
- **Why there is no deterministic keyword appraisal at turn start**: the chat model already reads the current user utterance when generating the reply, so same-turn emotional reaction belongs in that reply. `EmotionEngine` keeps cross-turn continuity (decay, conversation fatigue, post-turn classifier → next turn). The `CharacterState` prompt notes that affect is pre–current-utterance; end-of-turn expression follows the response via `resolve_expression` (LLM proposal canonical). Affect-to-expression mapping is not hardcoded: each card expression carries its own affect annotation (`extensions.ene.expressions[].affect`) and the arbiter picks the nearest annotated expression in affect space, so cards can define any expression names (Japanese, `x_` custom, …) without normalization tables. With the affect classifier disabled or failing, affect moves only via decay and long-conversation fatigue, so expressions stay effectively neutral unless the model itself emits explicit `<|perf:expr=…|>` markers.
- **Why prompt composition is a distinct, window-aware step** rather than a flat message array: sections (identity, affect, recalled memories, tool specs, dialogue history) compete for the model's context window, and packing order matters for keeping identity/safety rules intact under pressure — packing fills the window in priority order, always keeping required sections and dropping the lowest-priority droppable ones first (#370). See `docs/concepts/turn-and-session.md` §3 for the section layout.
- **Why the proactive decision context is serialized as JSON, not hand-assembled `key: value` text** (#380): the decision model's output is a JSON object with control fields (`should_speak`, `confidence`, …), and the context used to be free text in the same `key: value` shape. Third-party content absorbed into `screen_summary` or conversation history (web pages, documents, chats) could then carry lines that mimic those control fields and steer the judgment model. Serializing the context with `serde_json` embeds that content as escaped JSON *values* — structurally incapable of appearing as sibling control fields — and the decision system prompt explicitly labels `screen_summary`, `recent_conversation`, and activity labels as observation data that must never be read as instructions.
- **Why topic-boundary detection uses a centroid + composite score rather than consecutive-utterance similarity** (#367): pairwise similarity fails on backchannels, on short utterances with unstable embeddings, and on gradual drift that never produces a single low-similarity pair. A moving-average topic centroid accumulates drift, and a composite of centroid distance, silence, and topic turn count (with short backchannels excluded from both scoring and centroid updates) detects boundaries robustly across languages without any hard-coded keyword list. Detection only produces a signal/score; acting on it is deferred to compression (#368) and session splitting (#369) — see `docs/concepts/turn-and-session.md` §2.
- **Why tool-derived memories get a stable-key supersede and a validity gate (#349)**: tool-failure `Reflection` records are written on every failed call with volatile error text, so the content-based duplicate and semantic-match breakwaters never fire and the records accumulate without bound. The arbiter now keys tool-derived candidates on their stable `(kind, title)` pair and supersedes the prior active record on a match, and rejects candidates whose named tool was never invoked or whose content is pure boilerplate — see `docs/concepts/memory-system.md` §5.
- **Why the Identity Kernel budget scales with the model window and counts CJK denser (#386)**: a fixed 400-token cap and an English `chars / 4` truncation rule under-budget Japanese character cards by 3–4×, so the kernel was truncated far more aggressively than the rest of the prompt. The budget is now `available_window / 8`, clamped between 400 and 4 000 tokens, and truncation uses a language-aware per-character weight (CJK ≈ 1.5 chars/token, Latin ≈ 4 chars/token) so Japanese identity text is sized realistically.
- **Why the kernel's speech-style line comes from the card, not keyword inference (#388)**: the `Speech style` line used to sniff English length keywords (`short`/`brief`/`overlay`/`concise`) and otherwise surface the system prompt's first 160 characters as the character's speech style — Japanese cards were never detected, unrelated instructions leaked into the line, and the detected phrase was an English fixed sentence. The line now renders the card's structured `extensions.ene.speech` definition (length, first/second person, politeness, verbal tics) when present, and a concise default otherwise; labels and the default follow the app language so a Japanese character's prompt never receives an English speech-style sentence.

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
