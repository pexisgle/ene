# Issue #717

docs(product): track harness v1.0 done criteria to real-process verification

Closes #717

- Distinguish mechanism detection from real-process verification per done.md / w7-verification.md
- Map each unchecked v1.0 item to an implementation issue or explicit out-of-scope with successor milestone
- Require fmt/clippy/test/doc gates and EN/JA doc sync

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #717
- lint-safe, docs/ja sync where applicable
