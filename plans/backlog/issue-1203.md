# Issue #1203

feat(computer): observation-bound PC control with Task Grant

Closes #1203

- Add WindowIdentity/Observation ID, UIA backend, screenshot+element tree generation, stale-safe click/type/key/scroll, postcondition evaluator, semantic risk, Task-scoped Grant and hard confirmation with audit trace

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1203
- lint-safe, docs/ja sync where applicable
