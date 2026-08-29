# Issue #1181

fix(stage): responsive layout, contrast and raw-value isolation (follow-up to #1216)

Closes #1181

- Follow up to #1216: virtualize provider model picker, ensure Detail/Home scroll reachability, verify WCAG contrast per theme, move raw IDs/counters/schema/paths to Advanced/Copy diagnostics, fold MCP descriptions

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1181
- lint-safe, docs/ja sync where applicable
