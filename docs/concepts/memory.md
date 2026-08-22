# Memory

Memory is explicit, typed, and inspectable. Conversation stays in the
append-only session log (`ene-session`). Facts live in `ene-companion`
(`companions.db`) so they can be listed, edited, or forgotten without
rewriting history.

## Typed memories

Each row has:

- **Kind** — `episodic`, `semantic`, `user_profile`, `preference`,
  `commitment`.
- **Scope** — `private` (the writing soul) or `shared`.
- **Source** — `extraction`, `user_stated`, `tool`, `import`, `shared`.
- **Confidence and salience** — both influence recall.
- **Journal** — create / update / forget / supersede / restore /
  expire / complete is append-only.

Forgotten rows stay in the journal. A contradictory write can supersede an
older row.

## How memories are created

After a turn, the companion memory writer:

1. Extracts structured signals (commitments, user-stated facts, tool
   outcomes). Regex patterns (`my name is`, `i like`, `remember that`,
   and an explicit ISO / `YYYY-MM-DD` due such as `by 2026-08-30`)
   are a fail-closed safety net. Relative dates (`tomorrow`, `next
   Friday`) are not parsed; the classifier may still emit a
   `commitment` row without a due.
2. When `ai.tasks.classifier` (or chat fallback) is bound, the auxiliary
   LLM returns JSON candidates. Its `scope` (`private` / `shared`) overlays
   matching deterministic rows. Classifier errors skip extra candidates;
   they do not drop the safety-net extract.
3. The arbiter scores duplicates and contradictions, then writes, rejects,
   or parks the candidate.

Affect is hybrid: user-utterance heuristics blend with a classifier
proposal when confidence is high enough (`mind.affect.classifier_min_confidence`).
Unconfigured classifiers fail closed (heuristics only). Each prompt applies
that state before generation and logs `companion.affect` into system context.

When `mind.memory_approval.require_approval` is true (the default), parked
candidates wait in the pending queue. Resolve them through
`/api/v1/memories/pending` or the matching `ene-ctl memory` commands.

## How memories are recalled

Each turn, recall in `ene-companion` scores title/content overlap, recency,
and salience. When an embedding query vector is present (a bound
`ai.tasks.embedding` or chat-task fallback), cosine against
`memories.embedding` is added to the same ranker. Auto-recall
(`RecallPrefetch`) and the surface tool `memory.recall` share that path.
Unconfigured embedding
keeps lexical recall: a query with no overlapping tokens returns no hits
even if vectors were stored earlier. Hits land on
`ene-kernel::ContextRegistry` as `memory.semantic`. Standing profile and
preference notes are `memory.user_profile`; open (unexpired) commitments
are `memory.commitments`. Reading a memory bumps `access_count`.

A commitment due is `expires_at` on the memory row. It does not create
a Work schedule. Spoken reminders and cron jobs stay on
`/api/v1/schedules` (see [Schedules](../guides/schedules.md)).

Open commitments are injected every turn. Past-due rows are forgotten
with journal action `expired`. Completing one from Stage (or
`PATCH /api/v1/memories/{id}` with `completed: true`) journals
`completed`.

## Forgetting

`mind.forgetting.*` decays salience. Forgotten rows drop out of normal
recall. Pinning is a user edit (raise salience / keep the row).

## Inspecting memory

```sh
ene-ctl memory list <soul>
ene-ctl memory edit <id> "<content>"
ene-ctl memory delete <id>
```

See the [Memory ledger guide](../guides/memory-ledger.md).
