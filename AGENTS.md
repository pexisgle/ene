# AGENTS.md

## Project Overview
This project is an AI assistant application written in Rust that merges VTuber-like characters with Agentic AI capabilities.

## 0. Crucial Behaviors

* **Check Documentation:** Before planning changes, always read the relevant files in `docs/` or `crates/` to confirm the current design.
* **Verify & Complete:** Before declaring a task finished, run `cargo clippy --workspace` and `cargo test --workspace` (on Linux, use the §3 direnv invocation). Finally, check the PR Verification Checklist (§8) to ensure all requirements are met.
* **Correct Fixes:** If a test or build fails, read the compiler errors carefully. Always clarify the cause and the reason, and create a fix plan before making corrections. If the error is environment-specific, ask the user how to fix it.
* **Follow the Recipes:** When asked to add tools, configs, or IPC messages, strictly follow the steps in **§6 Common Tasks**.
* **Handle Hooks Gracefully:** If a git commit fails due to `cargo-husky` pre-commit hooks, read the hook output, fix the formatting or linter errors, and try committing again before resorting to `--no-verify`.

## 1. Where to Look

| Purpose | Reference |
|---|---|
| Platform-specific notes | §3 Platform-Specific Notes |
| Understanding crate layout and architecture | §4 Architecture & Philosophy |
| Matching project style, error handling, and logging | §5 Code Style & Safety Guidelines |
| Adding/modifying tools, configurations, or characters | §6 Common Tasks |
| Submitting a PR / Git Workflow | §8 Git & PR Policy |

## 2. AGENTS.md vs Skills vs docs/

| Scope | Where it lives |
|---|---|
| **Project-specific conventions** | **AGENTS.md** (this file) |
| **Design & architecture tutorials** | `docs/` (English) and `docs/ja/` (Japanese) |
| **End-user quickstart** | `README.md` |

## 3. Platform-Specific Notes

* **Linux:** Uses `direnv` + Nix flake. Run Cargo from the workspace root (shell cwd is already the repo; do not prefix commands with `cd`). Confirm `cargo` is not on PATH (`command -v cargo`); when it is not, use:
 `direnv exec . cargo <command>`
* **Windows:** Uses Windows Named Pipes for IPC.

## 4. Architecture & Philosophy

### 4.1 Crate Splits
The workspace is highly granularly split to enforce strict boundaries and prevent circular dependencies (API v2).
* **`ene-runtime`**: Host facade (`EneHandle::open`, `TurnId`, chat events, diagnostics). Ties together mind, store, AI providers, and tool host.
* **`ene-mind`**: Cognitive turn pipeline (session, recall, affect, Performance arbitration, memory writing). Does **not** depend on `ene-runtime` or `ene-tool-host`.
* **`ene-store`**: Exclusively owns `sea-orm` SQLite operations. Does **not** depend on `ene-ai` or `ene-mind`.
* **`ene-ai`**: LLM + embedding providers (absorbs former `ene-provider` / `ene-embedding`).
* **`ene-tool`**: Facade re-exporting `ene-tool-proto` + `ene-tool-common` + `ene-tool-derive`. Preferred import for new tool binaries. Does **not** depend on runtime / mind / store.
* **`ene-tool-proto`**: Defines the IPC ABI (Requests/Responses). *Must not contain business or DB logic.*
* **`ene-tool-host`**: Orchestrates tools and IPC. Depends on the tool ABI crates. Does **not** depend on `ene-ai` or `ene-store` — Tool RAG lives in `ene-tool-rag`.
* **`ene-tool-rag`**: Tool RAG pipeline — multi-vector embedding, weighted field similarity, optional HyDE, optional LLM rerank. Depends on `ene-ai`, `ene-store`, and `ene-tool-proto`.
* **`ene-vrm`**: VRM rendering for desktop. Does **not** depend on mind / runtime.
* **Rule:** Do not merge crates arbitrarily. Tool binaries must be kept extremely lightweight and only link what is absolutely necessary (prefer `ene-tool`, or at most `ene-tool-proto` + `ene-tool-derive`).
* **Rule: Dependency Centralization:** All external dependencies used by crates under `crates/` must be declared in the root `[workspace.dependencies]` table and referenced via `{ workspace = true }` in each crate's `Cargo.toml`. Do not pin version numbers directly in individual crate manifests.

### 4.2 Asset Distribution Strategy
* **Static Assets** (Default characters, config templates, UI icons): Distributed alongside the executable. On first launch, `ene-config` copies them to the user data directory. To keep the distribution lightweight, `.gitignore`'d resources (such as `assets/models/` and generated `assets/schema/*.schema.json`) **must not be bundled** in the release package.
* **Dynamic Assets** (User databases, character prompts): Managed by `ene-config` and reside in OS-standard data directories (e.g., `%APPDATA%` on Windows, `~/.config` on Linux). 
* **Rule:** The workspace root `assets/` directory is for local development and default templates. Heavy binary models or generated schemas must be excluded from distribution.

### 4.3 Technology Stack
* **Backend:** `tokio` (Async), `tracing` (Structured Logging).
* **GUI (`ene-desktop`):** A custom rendering stack utilizing `winit` (Windowing) + `wgpu` (Graphics) + `egui` (UI). It uniquely relies on `bevy_ecs` purely for state management and scheduling, without the Bevy rendering engine.
* **Memory System:** `sea-orm` + `sqlite-vec` + `sea-orm-migration`.
* **Tool Sandbox:** Tools run as separate processes communicating via IPC named pipes (Windows) or Unix Domain Sockets (Linux).

## 5. Code Style & Safety Guidelines

* **Async:** `tokio` only. Do not use `async-std` or `smol`.
* **Error Handling:** 
  - Use `thiserror` for module-level enums (e.g., `ToolHostError`). Do not use `anyhow` at the library boundary.
  - **Avoid `unwrap()` and `expect()` outside of tests.** The workspace enforces `#![warn(clippy::unwrap_used)]` and `#![warn(clippy::expect_used)]`. Always propagate errors or handle them gracefully using typed errors.
* **Logging:** 
  - Use the `tracing` crate (`info!`, `warn!`, `error!`, `debug!`). **Never use `println!`.**
  - Always include structured context fields when appropriate to maintain machine-readable logs (e.g., `tracing::error!(component = "ToolHost", error = %e, "Failed to start")`).
* **Concurrency:** Prefer `parking_lot::RwLock` or `parking_lot::Mutex` over standard library primitives to avoid lock poisoning. Use `std::sync::OnceLock` for lazy static initializations.
* **Events & i18n:** Backend crates (`ene-runtime`, `ene-mind`) must emit events and status messages in **English** (as static constants or Enums). Localization is the responsibility of the frontend/UI layer:
  - `ene-desktop` translations are managed under `apps/ene-desktop/i18n/`.
  - `ene-cli` translations are managed locally within the CLI under `apps/ene-cli/i18n/`.
* **Visibility:** Default to `pub(crate)`. Only use `pub` when external consumers need it.
* **Comments:** Write `rustdoc` comments (`///`) for public APIs and complex logic. Re-exports must use `#[doc(no_inline)]`.

## 6. Common Tasks (Recipes)

### R1. Add a new tool
1. **Create:** `cargo new --bin tools/<name>`
2. **Implement:** `#[derive(ene_tool_derive::ToolSpec)]` on the args struct, then wrap one or more `ToolAction`s in a `ToolProvider` — use `ene_tool::ActionSetProvider` (or `ene_tool::prelude::*`) instead of hand-writing the dispatch loop.
3. **Wire up:** Call `run_tool_server(Box::new(provider)).await` from `ene-tool` / `ene-tool-proto` inside `main`. This is **not generic** — there is no `run_tool_server::<MyAction>()`; it always takes a boxed `dyn ToolProvider`.
4. **Document:** Add to a category in `docs/tools/` and `docs/ja/tools/`.
5. **Verify:** Run `cargo run -p ene-cli` -> `/tool list`.

### R2. Add a config field
1. Edit the struct in `crates/ene-config/src/config.rs` (`define_config!` macro).
2. Run `cargo run -p ene-cli` once to auto-regenerate `assets/settings.schema.json` and `character_settings.schema.json`. *(Note: These JSON files are gitignored. Do not commit or hand-edit them).*
3. Document in `docs/configuration/settings.md` (both English and Japanese).

### R3. Add an IPC request/response
1. Extend `IpcRequest` / `IpcResponse` in `crates/ene-tool-proto/src/ipc.rs`.
2. Bump `PROTOCOL_VERSION` **only** if the wire format is incompatible.
3. Handle the added variant in `ene-tool-host` and all tool binaries.

### R4. Add or modify localization (i18n) strings
1. **Desktop (`ene-desktop`)**:
    - Add or edit keys directly in the translation file located under `apps/ene-desktop/i18n/{lang}/ene_desktop.ftl`.
    - Use the `i18n_embed_fl::fl!(crate::i18n::loader(), "key-name")` macro to retrieve the localized string in your code.
2. **CLI (`ene-cli`)**:
   - Add or edit keys directly in the CLI's local Fluent files under `apps/ene-cli/i18n/{lang}/ene_cli.ftl`.
   - Use the `i18n_embed_fl::fl!(crate::i18n::loader(), "key-name")` macro inside the CLI code.

## 7. Configuration Data Flow

Loaded by `figment` in the following order: 1. Compile-time defaults -> 2. OS-standard User Config (or local `assets/settings.json`) -> 3. `ENE_` env vars.

## 8. Git & PR Policy

> **⚠️ Early Development Phase**
> The project is currently in the early development stage. **AI agents do not need to create branches or pull requests (PRs) at this time.** Direct commits to `main` are acceptable while the project is in this phase. The policies below (branch naming, PR checklist, etc.) will be enforced once the project transitions to a stable release milestone.

* **Branch Naming:** `<type>/<short-kebab-case>` (e.g., `feat/sandbox-gate`, `fix/cli-race`).
* **Commits:** Follow Conventional Commits (`feat: ...`, `fix: ...`, etc.).
* **Documentation:** English (`docs/`) and Japanese (`docs/ja/`) must be kept in sync within the same PR.

### 8.1 Pre-commit Hooks (cargo-husky)

* `cargo-husky` is declared in the workspace (`Cargo.toml` / `.cargo-husky/hooks/`) and auto-installs hooks into `.git/hooks/` on the first `cargo build` that pulls it in.
* **`pre-commit`** runs `cargo fmt --all` against staged `.rs` files and re-stages the changes. Use `git commit --no-verify` to skip per-commit, or use `HUSKY=0 cargo build` to skip all hooks.

### PR Verification Checklist

Before submitting a PR or finishing a coding task, verify the following:

* [ ] `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` are clean.
* [ ] `cargo test --workspace` passes.
* [ ] `sea-orm` migrations (Rust modules) are present, tested, and registered in the Migrator (if schema changed).
* [ ] Config-field changes do not require manual schema commits (auto-regenerated, gitignored).
* [ ] Public API or behavior changes have corresponding updates under `docs/`.
* [ ] Both English (`docs/`) and Japanese (`docs/ja/`) docs are updated for any user-visible change.
* [ ] Branch name and commit/PR title follow Conventional Commits.

## 9. Further Reading

See `docs/` for deep dives into Architecture, Memory, Tools (RAG, SDK, Sandbox), and Core (Streaming, Prompting).
