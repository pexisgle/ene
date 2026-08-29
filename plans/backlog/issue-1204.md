# Issue #1204

feat(companion): stage presence, memory and voice on Attention

Closes #1204

- Stage idle/look-at -> TTS/lip-sync -> memory scope/provenance -> affect->body -> Attention-aware proactive -> STT/barge-in/self-voice -> decay/reflection in order

## Plan
- Track: allocated per plans/issue-backlog-plan.md
- Verified: plans/issue-backlog-plan-verification.md
- Implementation order: Track A/B/C first, then D/E

## Architecture boundaries
- Respects ene-session kernel/companion/work/plane/fiber boundaries
- No unwrap/expect/panic in prod code; SAFETY comments for unsafe

## Done criteria
- PR created with Closes #1204
- lint-safe, docs/ja sync where applicable
