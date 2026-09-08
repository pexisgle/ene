# AGENTS.md

Repository-level guidance for coding agents working on the new implementation.
Retired code under `.old/` is reference only: never depend on it, never copy
behavior from it without a test, never treat it as requirements or design.

## Source of truth

- Desired behavior: `docs/requirements/` (product definition, requirements,
  acceptance). Design adds no product behavior.
- Internal design: `docs/design/` (architecture → critical-areas → subsystems
  → concrete). Obey each Step 13 artifact's fixed premises; file conflicts as
  Issues instead of overriding.
- Implementation order and gates: `docs/implementation/README.md`.
- Current implementation/API: source code, Cargo manifests, tests, rustdoc.

## Commands

No `default-members`: bare `cargo test` / `cargo clippy` cover the workspace.

| Purpose            | Command                                                  |
| ------------------ | -------------------------------------------------------- |
| Format             | `cargo fmt --all` (check: `cargo fmt --all -- --check`)  |
| Focused iteration  | `cargo check -p <pkg>` / `cargo test -p <pkg>`           |
| Full lint          | `cargo clippy --workspace --all-targets -- -D warnings`  |
| Full tests         | `cargo test --workspace`                                 |
| Docs               | `cargo doc --workspace --no-deps`                        |

## Rust conventions

- Async is Tokio.
- Library errors use `thiserror`; avoid bare `String` or `Box<dyn Error>` as
  public library errors.
- Diagnostics use structured `tracing`.
- Prefer the narrowest visibility; `pub(crate)` by default unless an API must
  be public.
- Keep shared dependencies in root `[workspace.dependencies]` when more than
  one package shares version policy.
- Workspace clippy policy is authoritative. Do not weaken lints to make
  unrelated code pass.
- Every `unsafe` block requires a preceding `// SAFETY:` comment that states
  the invariant making it sound.

## Comments

Comments explain what code cannot express: invariants, ordering constraints,
provenance, or why a simpler implementation is incorrect. Do not restate code,
leave changelogs in comments, or park commented-out code. Public rustdoc
documents contracts.
