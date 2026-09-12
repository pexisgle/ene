# AGENTS.md

Repository-level guidance for coding agents working on the new implementation.
Retired code under `.old/` is reference only: never depend on it, never copy
behavior from it without a test, never treat it as requirements or design.

## Source of truth

- Desired behavior: `docs/requirements/` (product definition, requirements,
  acceptance). Design adds no product behavior.
- Internal design: `docs/design/`. Follow the layer precedence stated in
  `docs/design/README.md`; file conflicts between layers as Issues instead of
  resolving them in code.
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
- Keep external dependency version policy in root `[workspace.dependencies]`.
- Extra restriction lints live in `[workspace.lints.clippy]`; do not enable
  `pedantic`/`cargo` wholesale.
- Every `unsafe` block requires a preceding `// SAFETY:` comment that states
  the invariant making it sound.
- Never route a production path through a throwaway fake or bypass an
  authority boundary "for now"; unimplemented behavior is an explicit
  unsupported/unavailable outcome, not a fake success.
- Keep domain outcomes (stale, deny, hold, unknown) distinct from technical
  errors and from success.
- Never re-execute an external effect automatically while its outcome is
  unknown.

## Implementation invariants

- Read-only operations must not perform unrelated durable mutations as an
  initialization side effect. Split startup, repair, sweep, or migration work
  from state opening when the caller does not require that mutation.
- A public `limit`, page size, or cursor should bound upstream work as far as
  practical, not only the final response. Avoid full scans, full decode, or
  unbounded allocation followed by truncation when the owning query/storage
  boundary can apply the bound directly.
- Atomic publication and concurrent-writer serialization are separate
  guarantees. Temp-file + sync + atomic rename can prevent torn publication,
  but a read-modify-write state still needs a single writer, lock, CAS, or
  transaction when concurrent writers could otherwise lose updates.

## Comments

Comments explain what code cannot express: invariants, ordering constraints,
provenance, or why a simpler implementation is incorrect. Do not restate code,
leave changelogs in comments, or park commented-out code. Public rustdoc
documents contracts. A comment can document an invariant but cannot enforce
one; correctness that depends on serialization, currentness, atomicity, or a
similar premise must be enforced by the owning API/code path and covered by a
test.
