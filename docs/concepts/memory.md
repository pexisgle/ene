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
- **Journal** — create / update / forget / supersede / restore is
  append-only.

Forgotten rows stay in the journal. A contradictory write can supersede an
older row.

## How memories are created

After a turn, the companion memory writer:

1. Extracts structured signals (commitments, user-stated facts, tool
   outcomes).
2. Optionally classifies more candidates when a classify model is wired.
3. The arbiter scores duplicates and contradictions, then writes, rejects,
   or parks the candidate.

When `mind.memory_approval.require_approval` is true (the default), parked
candidates wait in the pending queue. Resolve them through
`/api/v1/memories/pending` or the matching `ene-ctl memory` commands.

## How memories are recalled

Each turn, recall in `ene-companion` searches title/content (FTS) plus
salience, recency, and access count. Results are injected into the kernel
prompt. Reading a memory bumps `access_count`.

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
