# Issue #1209

docs(product): product-convergence implementation policy

Closes #1209

- Codify crate boundaries, typed state/error/ID, normal-table vs event-log, REST projection/WS diff, runner scope/cancel/verification, stale-safe actions, Grant, Attention delivery, copy/a11y, privacy-safe spans, Definition of Done

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1209
- lint-safe, docs/ja sync where applicable
