# Issue #1199

feat(companion): Attention and completion reporting pipeline

Closes #1199

- Add Attention Item/Store/state with priority/action_required/dedupe/expiry, task report adapter, quiet-hours/speaking gate
- Deliver via surface turn/speech/card/notification/digest and Task/Attention Center API without leaking raw runner text

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1199
- lint-safe, docs/ja sync where applicable
