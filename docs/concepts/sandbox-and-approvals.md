# Sandbox and approvals

Three layers keep tools from touching what they were not granted:

1. **OS sandbox** (`ene-sandbox`) — Landlock, seccomp, rlimits on Linux.
2. **Host mediation** (`ene-plugin-host`) — spawn, grants, and reversible
   dispose. `unload`, circuit trip, and loading rollback invert the same
   `Effect` stack LIFO. Tool names are stacked per fiber owner, so unloading
   one fiber cannot drop another fiber's live tool. Kill is not unload.
3. **Approval plane** (`ene-access-control`) — deny-by-default until a policy row
   matches; decisions are hash-chained in the audit log. `approval.mode =
   ai_auto` asks `ai.tasks.approve` (chat fallback); a failed or missing
   helper falls back to the popup and never auto-runs.

`fs.read` / `fs.write` are confined with parent-canonicalization (relative
`../` included). The tool workspace is `<data>/workspace`, not the data
directory, so `api.token` / `vault.key` / `sessions.db` are not auto-approved
read targets. Job working copies live at
`<data>/workspace/jobs/<soul_id>/<job_id>/`. Completing a job copies
registered artifacts to `<data>/workspace/jobs/<soul_id>/artifacts/` and
flips `delivered` on `GET /api/v1/artifacts`. That soul `artifacts/` tree
is not the fs/exec default scope.

Credentials live in the vault (`vault.bin` + `vault.key`), not in plugin
environment variables. Unknown tools with empty `side_effects` still
classify as medium sensitivity unless the registry knows them.

Known gaps (web plugin bypassing the net broker, FileBroker glob/delete,
`exec` process-tree limits) are tabulated in
[Product boundaries](product-boundaries.md).
