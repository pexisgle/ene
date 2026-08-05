# Memory ledger

The memory ledger is the user-facing view of everything the companion
remembers. It exists in two places: the desktop Settings → Memory ledger
page and the CLI `/memory` / `/commitments` commands.

## Desktop ledger

The Memory page has four tabs:

- **Browse** — all typed memories, filterable by kind/status/scope, with
  inline editing and pinning.
- **Recall** — what the hybrid search would return for a query, with the
  score breakdown (why this memory is relevant).
- **Pending** — candidates awaiting approval when
  `mind.memory_approval.require_approval` is on. Approve, reject, or edit
  before they become memories.
- **Commitments** — the commitment ledger (open/closed promises).

## CLI

```sh
/memory list
/memory search "camping"
/memory inspect <id>
/memory why <id>               # why a memory is recalled
/memory pin <id>
/memory archive <id>
/memory forget <id>            # mark as user-deleted
/memory dispute <id>
/memory restore <id>
/memory status
/memory pending
/memory approval list|inspect|approve <id>|reject <id>|edit <id>|history
/commitments list
/commitments done <id>
/affect show
```

## What you can do

| Action | Where | Effect |
|---|---|---|
| Approve/reject a pending candidate | Desktop pending tab; lifecycle events notify you | Candidate becomes a memory or is discarded |
| Edit a memory | Desktop ledger / `/memory approval edit <id>` | Content, kind, salience, confidence (candidates awaiting approval) |
| Pin a memory | Desktop / `/memory pin <id>` | Exempt from natural decay |
| Remove a memory | Desktop / `/memory forget <id>` | Marked `user_deleted` (audited); `/memory restore <id>` brings it back |
| Close a commitment | `/commitments done <id>` | Leaves the active prompt |
| Reset affect | `/affect reset` | Emotion state back to baseline |

## How memory behaves afterwards

- Edited/salience-adjusted memories emit `MemoryLedgerChanged` lifecycle
  events so the UI stays in sync.
- Pinned memories never fade; everything else decays (see
  [Memory → Forgetting](../concepts/memory.md#forgetting-and-the-lifecycle)).
- If you delete something the companion later re-extracts, the new
  candidate will come back through the pipeline (you can use
  `user_deleted`/dispute semantics or keep approval mode on).

## Approval workflow configuration

```json
{
  "mind": {
    "memory_approval": { "require_approval": true }
  }
}
```

With approval on, no extracted memory activates without your review —
recommended if you want full control over what the companion remembers.
