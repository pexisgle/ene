# Issue #1208

docs(product): reclassify P-requirements and update v1 definition

Closes #1208

- Reclassify P-1xx..P-10xx into V1-Core/V1-Safety/Presence/Learning/Later/Form-only, assign new P-numbers for Task Contract/Attention/Grant/Computer Action/Learning Candidate, sync features.md/done.md, forbid Later->v1 without decision

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1208
- lint-safe, docs/ja sync where applicable
