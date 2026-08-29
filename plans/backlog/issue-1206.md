# Issue #1206

docs(product): real-model E2E and safety verification harness

Closes #1206

- Define verification for research->Attention flow, parallel conversation, follow-up/cancel/failure/restart, window/modal/focus/stale, quiet-hours, candidate/canary, accessibility, perf baseline, scope/grant/injection with real provider/Windows/UIA

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1206
- lint-safe, docs/ja sync where applicable
