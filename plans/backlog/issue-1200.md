# Issue #1200

docs(product): formalize product decisions and v1 boundary

Closes #1200

- Formalize PC-D1..D6 into product/vision.md and decisions.md with D-numbers, fix initial OS/model/voice/multi-companion/Web boundary
- Align vision.md/features.md/done.md/decisions.md to same v1 described by ~10 observable experiences

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1200
- lint-safe, docs/ja sync where applicable
