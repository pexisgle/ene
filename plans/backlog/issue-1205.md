# Issue #1205

feat(work): safe self-evolution with skill evaluation and rollback

Closes #1205

- Add Learning Candidate store/state, correction detector, draft generator, static validator, replay evaluator, approval UI, versioned activation/canary/rollback with permission diff and plane bypass guard

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1205
- lint-safe, docs/ja sync where applicable
