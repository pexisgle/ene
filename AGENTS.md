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

## Test suite design and maintenance

* Add a test only for a distinct contract, boundary, failure mode, or regression that no existing test owns. Before adding it, locate the nearest owner and state which unique assertion the new test contributes.
* Keep pure local invariants in focused unit tests. Use integration and end-to-end tests for composition, durability, authority, WSS, process isolation, and native boundaries; do not duplicate a lower-layer suite at every layer.
* Use table-driven cases when only input or presentation varies within the same contract. Keep separate declarations for independent races, protocol directions, lifecycle phases, and technical-versus-domain outcomes.
* Merge tests only when their setup and contract are the same and every unique assertion moves to the surviving test; moving subcases without reducing coverage is not a reduction.
* Every test must verify an observable oracle: a typed result, durable state, side-effect presence or absence, cleanup, or boundary rejection. Do not use an internal counter or a test-only hook as the sole proof unless that counter is itself the contract.
* Keep test-only support narrowly feature- or test-gated, and remove gates, injectors, fixtures, and other scaffolding when no retained test needs them.
* Count executable test attributes rather than textual matches: comments and lint reasons may contain `#[test]`, and variants such as `#[tokio::test(start_paused = true)]` are executable tests too.
* For a broad test-suite reduction, use a fresh independent review after each change. Do not declare the suite minimal until three consecutive independent reviews find no safe merge or deletion candidate.

## Agent workflow

* Prefer narrow symbol queries over bulk-reading source files.
  Use exact-text search for literals, configuration, and protocol strings.
* Verify cross-crate architecture or impact claims against source,
  the compiler, and tests.
* Read only the relevant files and ranges needed for the task.

## Comments and documentation

Write self-documenting code. Keep comments and rustdoc to the absolute
minimum: explain only non-obvious rationale that cannot be expressed in code.
Do not restate the code, write obvious docs, leave changelogs, or keep
commented-out code.

A comment cannot enforce correctness; enforce invariants through types, APIs,
and tests.
