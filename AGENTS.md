# AGENTS.md

Repository-level guidance for coding agents working on the new implementation.

Backward compatibility is out of scope. Do not add migration layers, shims,
legacy workarounds, or compatibility code for previous implementations.
Retired implementations are not kept in the repository and are never
authoritative: do not preserve behavior merely because it existed there.
Validate behavior against the current requirements and design.

## Source of truth

* Product behavior and acceptance: `docs/requirements/`. Design and
  implementation must not add product behavior that is absent from the
  requirements.
* Internal design: `docs/design/`. Follow the precedence in
  `docs/design/README.md`. If design layers conflict, do not resolve the
  conflict locally in code; surface it as an Issue or design change first.
* Implementation order and stage gates: `docs/implementation/README.md`.
* Current milestone and progress only: `docs/implementation/PROGRESS.md`.
  It is an index, not an implementation contract.
* Current implementation and API shape: source code, Cargo manifests, tests,
  and rustdoc.

## Commands

There are no `default-members`; workspace-root commands cover all workspace
members unless explicitly narrowed.

| Purpose       | Command                                                 |
| ------------- | ------------------------------------------------------- |
| Format        | `cargo fmt --all`                                       |
| Format check  | `cargo fmt --all -- --check`                            |
| Focused check | `cargo check -p <pkg>`                                  |
| Focused tests | `cargo test -p <pkg>`                                   |
| Full lint     | `cargo clippy --workspace --all-targets -- -D warnings` |
| Full build    | `cargo build --workspace`                               |
| Full tests    | `cargo test --workspace`                                |
| Docs          | `cargo doc --workspace --no-deps`                       |

CI performs the corresponding checks with the committed `Cargo.lock` locked.
The Clippy gate uses `-D warnings`; do not leave Rust or Clippy lint warnings
in completed changes.

## Rust conventions

* Async is Tokio.
* Public library errors use `thiserror`; do not expose bare `String` or
  `Box<dyn Error>` as library error types.
* Prefer the narrowest visibility; use `pub(crate)` unless an API must cross
  the crate boundary.
* Keep external dependency version policy in root `[workspace.dependencies]`;
  workspace crates should inherit shared dependencies and lints.
* Extra restriction lints belong in `[workspace.lints.clippy]`; do not enable
  `pedantic` or `cargo` wholesale.
* `unwrap`, `expect`, and `panic` are denied in production code by workspace
  lints. Tests may use them where permitted by `clippy.toml`.
* Every `unsafe` block requires a preceding `// SAFETY:` comment stating the
  invariant that makes it sound.
* Never route a production path through a throwaway fake or bypass an
  authority boundary "for now". Unimplemented behavior must produce an
  explicit unsupported or unavailable outcome, not fake success.
* Keep domain outcomes such as stale, deny, hold, and unknown distinct from
  technical errors and from success.
* Never automatically re-execute an external effect while its outcome is
  unknown.

## Cross-cutting correctness

* Read-only operations must not perform unrelated durable mutations as an
  initialization side effect. Separate opening from startup, repair, sweep,
  migration, or other mutating maintenance.
* Public limits, page sizes, and cursors should bound upstream work at the
  owning query or storage boundary where practical. Do not perform an
  avoidable full scan, decode, or allocation and truncate only afterward.
* Atomic publication and concurrent-writer serialization are different
  guarantees. Atomic rename can prevent torn publication, but read-modify-write
  state still requires a single writer, lock, CAS, or transaction where
  concurrent writers could lose updates.

## Agent workflow

* Prefer narrow semantic or symbol queries over bulk-reading source files.
  Use Serena for definitions, references, and implementations when available;
  use exact-text search for literals, configuration, and protocol strings.
* Use repository graph/index tools for cross-crate architecture or impact
  analysis when available, but verify inferred relationships against source,
  semantic tooling, the compiler, and tests.
* Read only the relevant files and ranges needed for the task.

## Comments and documentation

Comments and rustdoc should explain contracts, invariants, ordering,
provenance, or why a simpler implementation is incorrect. Do not restate the
code, leave changelogs in comments, or keep commented-out code.

A comment may document an invariant but cannot enforce it. Correctness that
depends on serialization, currentness, atomicity, or a similar premise must be
enforced by the owning API or code path and covered by tests.
