# Issue #1202

docs(stage): rework stage IA toward product convergence

Closes #1202

- Reorganize stage around Conversation / Tasks & Attention / Companion, show active soul consistently, add setup wizard and scoped approval, separate Settings/Diagnostics, move raw IDs to Advanced, ensure keyboard/UIA/EN-JA parity

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1202
- lint-safe, docs/ja sync where applicable
