# Memory ledger

The ledger is the inspectable view of `ene-companion` memories. Stage's
Memory tab lists rows (commitments first, with due, linked schedule, and
Complete). `ene-ctl` and HTTP remain the other surfaces.

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
| Complete a commitment | Stage Complete, or `PATCH` with `completed: true` | Forgotten; journal action `completed`; linked schedule disabled |
| Link a Work schedule | `PATCH` with `schedule_id` | Commitment-only; same soul; empty string clears without disabling |
| Forget a memory | `ene-ctl memory delete` | Forgotten flag; journal keeps the action; linked schedule disabled |
| Resolve a pending candidate | pending / resolve endpoints | Write, reject, or edit before it becomes a row |

How extraction and recall work is in [Memory](../concepts/memory.md).
