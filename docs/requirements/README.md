# Requirements

This directory is the source of truth for **what Ene should become**.

Requirements are currently being rebuilt interactively. A document being present here does not mean every section is decided: explicit `TBD` items are unresolved, and only entries marked as confirmed/accepted are normative.

## Documents

- [Product](product.md) — what Ene is, target users, goals, non-goals
- [Use cases](use-cases.md) — observable user scenarios
- [Functional requirements](functional.md) — capabilities Ene must provide
- [Non-functional requirements](non-functional.md) — performance, reliability, privacy, portability, etc.
- [Invariants](invariants.md) — rules no implementation may violate
- [Glossary](glossary.md) — precise domain vocabulary
- [Decisions](decisions.md) — confirmed product/requirement decisions and rationale
- [Legacy](legacy/README.md) — pre-reset documents kept only as historical input

## Rule of interpretation

1. Confirmed requirements and decisions in this directory define desired behavior.
2. Code/rustdoc define current behavior, not desired behavior.
3. `docs/concepts/`, `docs/apps/`, `docs/guides/`, and `docs/reference/` describe the current implementation.
4. `legacy/` is never authoritative. Reconfirm a legacy idea before copying it into current requirements.
5. Old implementation plans are kept in Git history instead of the working tree.
