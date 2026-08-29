# Issue #1207

docs(product): sequential migration and rollout for breaking changes

Closes #1207

- Order docs/contracts -> new types -> DB migration -> runner -> API/SDK -> stage -> computer -> presence/voice -> self-evolution -> old path removal, forbid old job runtime coexistence, ensure backup restore and Interrupted reporting

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1207
- lint-safe, docs/ja sync where applicable
