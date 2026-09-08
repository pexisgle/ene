# AGENTS.md

This file provides repository-level implementation guidance for coding agents.

## Release status

This app is **unreleased** and has no external consumers, so backward compatibility is not a constraint. Do not preserve config schemas, IPC/plugin protocols, persisted data, or CLI surfaces for compatibility's sake; prefer clean designs.

## Requirements reset

Product requirements and architecture are being redefined. `docs/requirements/` is the only working-tree location for desired product behavior. Requirements documentation is temporarily maintained in Japanese only while the specification is being rebuilt. Do not create or maintain a parallel English copy until the requirements stabilize.

Historical design and planning documents have been removed from the working tree; Git history may be consulted as non-authoritative input when explicitly useful.

Do not infer requirements from existing implementation. Existing code describes what currently happens, not necessarily what should happen. When a requirement is not confirmed, leave it undecided rather than reconstructing an old design from code or history.

## Build environment

Linux native dependencies come from the checked-in Nix flake. If `direnv` is active, plain `cargo` works; otherwise use `nix develop --command`. Native Windows development uses stable MSVC Rust, Visual Studio C++ Build Tools, and the Windows SDK.

## Commands

`default-members = ["apps/ene-ctl"]`, so bare `cargo test` / `cargo clippy` does not cover the workspace.

| Purpose | Command |
|---|---|
| Format | `cargo fmt --all` (check: `cargo fmt --all -- --check`) |
| Focused iteration | `cargo check -p <pkg>` / `cargo test -p <pkg>` |
| Full lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Full tests | `cargo test --workspace` |
| Docs | `cargo doc --workspace --no-deps` |

## Rust conventions

- Async is Tokio.
- Library errors use `thiserror`; avoid bare `String` or `Box<dyn Error>` as public library errors.
- Diagnostics use structured `tracing`.
- Prefer the narrowest visibility; `pub(crate)` by default unless an API must be public.
- Keep shared dependencies in root `[workspace.dependencies]` when more than one package shares version policy.
- Workspace clippy policy is authoritative. Do not weaken lints to make unrelated code pass.
- Every `unsafe` block requires a preceding `// SAFETY:` comment that states the invariant making it sound.

## Comments

Comments should explain information the code cannot express: invariants, ordering constraints, provenance, or why a simpler implementation is incorrect. Do not restate code, leave changelogs in comments, or park commented-out code. Public rustdoc documents contracts.

## Source of truth

- Desired behavior: explicitly confirmed requirements under `docs/requirements/`.
- Current implementation/API: source code, Cargo manifests, tests, and rustdoc.
- Historical rationale: Git history, only as supporting evidence and never as current specification by itself.

Implementation documentation should be added only after the relevant requirement/design is confirmed and the implementation actually matches it.
