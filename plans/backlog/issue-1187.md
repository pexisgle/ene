# Issue #1187

docs(product): track harness v1.0 blocking items with observable acceptance

Closes #1187

- Enumerate offline GGUF, barge-in, self-voice, two-body, VRM lip-sync, job cancel/follow-up, compaction LLM, report gating, MCP/skill, fs/exec, task speech, audit AI, offline-zero
- Define observable acceptance (real provider/process/store/HTTP/WS or manual GUI with env/steps/result)

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1187
- lint-safe, docs/ja sync where applicable
