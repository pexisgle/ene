# Stage Readiness Probe Fix (#1177)
> Follow-up to #1211 (readiness gaps).
- Derive Companion/Voice/Home readiness from probeable minimal conditions, not just saved values
- Badge active companion, disable current action
- Guard Mic toggle when STT unconfigured: show Voice CTA, don't turn ON
- Distinguish activate/import/install success messages
- Keep Home/Detail/soul consistent after restart
Covered by probe tests in ene-stage.
