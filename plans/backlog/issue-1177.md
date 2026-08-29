# Issue #1177

fix(stage): readiness derived from probe and active companion (follow-up to #1211)

Closes #1177

- Follow up to #1211: derive Companion/Voice/Home readiness from probeable minimal conditions, badge active companion, guard Mic toggle when STT unconfigured with Voice CTA, distinguish activate/import/install messages, keep Home/Detail/soul consistent after restart

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1177
- lint-safe, docs/ja sync where applicable
