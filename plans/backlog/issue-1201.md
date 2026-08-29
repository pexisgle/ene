# Issue #1201

feat(product): complete the conversation-while-research vertical slice

Closes #1201

- Wire VS-01..VS-07 so real model + real web/fs tools produce Markdown, conversation stays on same soul, follow-up/question/cancel reach runner, completion via Attention, artifacts openable from Task Center

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1201
- lint-safe, docs/ja sync where applicable
