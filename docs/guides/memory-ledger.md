# Memory ledger

The ledger is the inspectable view of `ene-companion` memories. There is no
in-process Settings page for it: use the HTTP API or `ene-ctl`.

## CLI

```sh
ene-ctl memory list <soul>
ene-ctl memory edit <id> "<content>"
ene-ctl memory delete <id>
```

Pending candidates (when `mind.memory_approval.require_approval` is on) are
`GET /api/v1/memories/pending` and
`POST /api/v1/memories/candidates/{id}/resolve`.

## What you can do

| Action | Where | Effect |
|---|---|---|
| List memories for a soul | `ene-ctl memory list` | Rows from `companions.db` |
| Edit content / scope | `PATCH /api/v1/memories/{id}` | Journaled update |
| Forget a memory | `ene-ctl memory delete` | Forgotten flag; journal keeps the action |
| Resolve a pending candidate | pending / resolve endpoints | Write, reject, or edit before it becomes a row |

How extraction and recall work is in [Memory](../concepts/memory.md).
