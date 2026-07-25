# Agent Instructions

## Scope

- These are repository-wide defaults. A nearer `AGENTS.md`/`AGENTS.override.md` may narrow them; the user's request wins.
- Inspect `git status` first and preserve unrelated user changes. Do not reset, overwrite, commit, branch, or open a PR unless explicitly asked.
- Prefer the smallest focused change. Treat current code/tests and accepted ADRs as evidence when documentation conflicts; do not copy stale examples.

## Source of truth

This is a Rust 2024 Cargo workspace. `Cargo.toml` includes `crates/*`, `plugins/*`,
`plugins/tool/*`, and `apps/*`; `apps/ene-cli` is the default member, so commands
without `--workspace` do not cover the whole repository.

| Need | Start here |
|---|---|
| Setup | `README.md`, `docs/getting-started.md` |
| Config/paths | `docs/configuration.md`, `crates/ene-config/src/` |
| Architecture/API | `docs/architecture.md`, `docs/crates/` |
| Runtime/events | `docs/concepts/turn-and-session.md`, `docs/crates/runtime.md` |
| Plugins/Tools/IPC | `docs/concepts/plugins-and-mcp.md`, `docs/crates/plugin-system.md`, `docs/crates/tool-sdk.md` |
| Apps | `docs/apps/cli.md`, `docs/apps/desktop.md` |
| Japanese user docs | Matching files under `docs/ja/` |

Read relevant docs, the affected crate's `Cargo.toml`, public API, tests, and call
sites before planning a non-trivial change.

## Environment and validation

Run commands from the repository root. On Linux use the checked-in Nix/direnv
environment: prefer `cargo` when available, otherwise `direnv exec . cargo ...` or
`nix develop --command cargo ...`. Do not add `cd` to repository-root commands.

| Purpose | Command |
|---|---|
| Format check | `cargo fmt --all -- --check` |
| Focused compile/test | `cargo check -p <package>` / `cargo test -p <package>` |
| Workspace lint | `cargo clippy --workspace -- -D warnings` |
| Workspace tests | `cargo test --workspace` |
| CLI / desktop | `cargo run -p ene-cli -- --help` / `cargo run -p ene-desktop` |

Use focused checks while iterating, then run workspace lint/tests for code changes;
use `--all-targets` when tests/examples/non-library targets are affected. Report the
failing package/target and root cause; do not hide failures by relaxing lints.

## Architecture boundaries and API v1

- `ene-runtime` is the host/actor facade and bootstrap layer. `ene-mind` owns session, recall, prompt composition, affect, performance, and memory writing; it must not depend on runtime or tool-host.
- `ene-store` alone owns SQLite/SeaORM connections, schema, migrations, and raw DB access. It must not depend on AI/mind; callers use its public API. Stateful tool binaries use `ene-tool-db` over IPC.
- `ene-ai` owns LLM/embedding providers. `ene-tool-rag` owns retrieval; `ene-plugin-proto` is wire ABI only (protocol v3); `ene-plugin` is the authoring facade; `ene-plugin-host` owns process/registry orchestration for all plugins (tools, providers, MCP); `ene-vrm` is rendering-only.
- Keep `plugins/tool/*` lightweight separate binaries. Do not add arbitrary cross-crate dependencies or move business/DB logic into ABI crates.
- Preserve API v1: every turn has a `TurnId`; `run` is single-flight and returns `Busy`; `Terminal` follows history commit and synchronous finalization; deferred memory work may continue; `Performance` is the presentation event; detailed pipeline diagnostics are separate from the chat bus.
- Keep dependencies in root `[workspace.dependencies]` for workspace crates and use `{ workspace = true }`; do not silently change versions.

## Rust, safety, and generated data

- Use Tokio for async code and `thiserror` for library errors. Do not expose `anyhow`, bare `String`, or `Box<dyn Error>` as library boundaries.
- Avoid `unwrap`, `expect`, and panic paths in production. Tests may use them only under existing scoped lint expectations; production exceptions need narrow `#[expect(..., reason = "...")]`.
- Use structured `tracing` for library/runtime diagnostics. CLI output/examples may use stdout/stderr; do not use `println!` for library logging.
- Prefer `parking_lot` for internal locks where compatible, `OnceLock` for one-time initialization, and the narrowest visibility (`pub(crate)` by default). Comments explain non-obvious why; public API changes need rustdoc and reference docs.
- Configuration is defaults → JSON → `ENE_` environment variables (`__` separates nested keys, e.g. `ENE_AI__TASKS__CHAT__MODEL`). Current public sections are `ai.*`, `store.*`, `mind.*`, `plugins.*`, and `desktop.*`; plugin entries are `plugins.list.<name>` with flattened fields.
- Add settings at the owning `define_config!` invocation, which may be outside `ene-config`. Regenerate schemas through the CLI; never hand-edit or commit ignored `assets/schema/*`.
- Never commit/log secrets, `.env`, `memory.db*`, `undo.db*`, `todo.db*`, downloaded model weights, or heavy ignored assets. `assets/` is for development/default resources.

## Plugins, Tools, IPC, and localization

- New tools are plugins: `cargo new --bin plugins/tool/<name>`; derive `ToolAction`; prefer `ene_tool_common::ActionSetProvider`/`prelude`; wrap with `ene_plugin::ToolPluginAdapter` and serve with `run_plugin_server(Box::new(ToolPluginAdapter(provider))).await`; use `ene-tool-db` for state; use namespaced `<namespace>.<action>` names; declare side effects/sandbox needs.
- Verify tool binaries with `/tool list` and update both `docs/concepts/plugins-and-mcp.md` and `docs/ja/concepts/plugins-and-mcp.md`.
- IPC work starts at `crates/ene-plugin-proto/src/ipc.rs` (protocol v4). Preserve length-prefixed JSON, update host/plugins/tests, and bump `PLUGIN_IPC_PROTOCOL_VERSION` only for intentional wire incompatibility.
- Backend events/statuses stay stable English contracts. UI strings belong in `apps/ene-desktop/i18n/{en-US,ja}/ene_desktop.ftl` and `apps/ene-cli/i18n/{en-US,ja}/ene_cli.ftl`; keep EN/JA user docs synchronized.

## Completion

1. Inspect status, relevant docs/manifests/code/tests/generated-file rules.
2. Make the focused change; update regression tests and public docs when behavior/API changes.
3. Run formatting, focused checks, then required workspace checks. Review the final diff and confirm no secrets, generated files, or user changes were touched.
4. Report exact changed paths and validation results. Do not claim an unrun check passed.

Do not bypass hooks or use `--no-verify` to conceal failures. `cargo-husky` formats
staged Rust files; inspect any resulting diff. When commits are explicitly requested,
use Conventional Commits and include required documentation updates.
