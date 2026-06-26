# AGENTS.md — ene

## 0. Purpose & AI Directives

This file is the source of truth for **project-specific conventions** in the `ene` workspace.

**For AI Agents (Crucial Behaviors):**
* **Search First:** Always read the relevant files in `docs/` or `crates/` before proposing large architectural changes.
* **Verify & Complete:** Before declaring a task finished, automatically run `cargo clippy --workspace` and `cargo test --workspace`. Finally, mentally verify the PR Verification Checklist (§8) and confirm all requirements are met.
* **No Hallucinated Fixes:** If a test or build fails, read the compiler errors carefully. Do not blindly guess fixes; ask the user for context if the error is environment-specific.
* **Follow the Recipes:** When asked to add tools, configs, or IPC messages, strictly follow the steps in **§6 Common Tasks**.
* **Handle Hooks Gracefully:** If a git commit fails due to `cargo-husky` pre-commit hooks, read the hook output, fix the formatting or linting errors, and try committing again before resorting to `--no-verify`.

## 1. Where to Look

| You want to… | Go to |
|---|---|
| Set up a dev environment | §3 Platform Setup |
| Understand crate layout or architecture | §4 Architecture & Philosophy |
| Match project style, error handling, or logging | §5 Code Style & Safety |
| Add / modify a tool, config, or character | §6 Common Tasks |
| Submit a PR / Git Workflow | §8 Git & PR Policy |

## 2. AGENTS.md vs Skills vs docs/

| Scope | Where it lives |
|---|---|
| **Project-specific conventions** | **AGENTS.md** (this file) |
| **Language-general advice** (Rust, testing) | `.opencode/skills/` (Link to these by name, do not duplicate here) |
| **Design & architecture tutorials** | `docs/` (English) and `docs/ja/` (Japanese) |
| **End-user quickstart** | `README.md` |

## 3. Platform Setup

* **Linux (Recommended):** Uses `direnv` + Nix flake (pins nightly Rust, mold, clang, GTK3, Wayland, Chromium, etc.). Run `direnv allow` followed by `cargo build`.
* **Windows (Community):** Requires Visual Studio Build Tools, Rust nightly, WebView2, and OpenSSL (or `rustls`). IPC uses Windows Named Pipes.

## 4. Architecture & Philosophy

### 4.1 Domain-Driven Crate Splits (Strict Boundaries)
The workspace is highly granular to enforce strict boundaries and prevent circular dependencies.
* **`ene-tool-proto`**: Defines the IPC ABI (Requests/Responses). *Must never contain business or DB logic.*
* **`ene-tool-host`**: Orchestrates tools and IPC. Depends on proto.
* **`ene-core`**: The central facade tying together memory, tools, providers, and embedding.
* **`ene-memory`**: Exclusive owner of `sea-orm` SQLite operations.
* **Rule:** Do not merge crates arbitrarily. Tool binaries must remain extremely lightweight and only link what they absolutely need (typically just `ene-tool-proto` and `ene-tool-derive`).

### 4.2 Asset Distribution Strategy
* **Static Assets** (Default characters, config templates, UI icons): Distributed alongside the executable. On first launch, `ene-config` copies them to the user data directory. To keep the distribution lightweight, `.gitignore`'d resources (such as `assets/models/` and generated `assets/schema/*.schema.json`) **must not be bundled** in the release package.
* **Dynamic Assets** (User databases, character prompts): Managed by `ene-config` and reside in OS-standard data directories (e.g., `%APPDATA%` on Windows, `~/.config` on Linux). 
* **Rule:** The workspace root `assets/` directory is for local development and default templates. Heavy binary models or generated schemas must be excluded from distribution.

### 4.3 Technology Stack
* **Backend:** `tokio` (Async), `tracing` (Structured Logging).
* **GUI (`ene-desktop`):** A custom rendering stack utilizing `winit` (Windowing) + `wgpu` (Graphics) + `egui` (UI). It uniquely relies on `bevy_ecs` purely for state management and scheduling, without the Bevy rendering engine.
* **Memory System:** `sea-orm` + `sqlite-vec` + `sea-orm-migration`. *Never use diesel or rusqlite directly.*
* **Tool Sandbox:** Tools run as separate processes communicating via IPC named pipes (Windows) or Unix Domain Sockets (Linux).

## 5. Code Style & Safety Guidelines

* **Async:** `tokio` only. No `async-std` or `smol`.
* **Error Handling:** 
  - Use `thiserror` for module-level enums (e.g., `ToolHostError`). No `anyhow` at the library boundary.
  - **Avoid `unwrap()` and `expect()` outside of tests.** The workspace enforces `#![warn(clippy::unwrap_used)]`. Always propagate errors or handle them gracefully using typed errors.
* **Logging:** 
  - Use the `tracing` crate (`info!`, `warn!`, `error!`, `debug!`). **Never use `println!`.**
  - Always include structured context fields when appropriate to maintain machine-readable logs (e.g., `tracing::error!(component = "ToolHost", error = %e, "Failed to start")`).
* **Concurrency:** Prefer `parking_lot::RwLock` or `parking_lot::Mutex` over standard library primitives to avoid lock poisoning. Use `std::sync::OnceLock` for lazy static initializations.
* **Events & i18n:** Backend crates (`ene-core`, `ene-session`) must emit events and status messages in **English** (as static constants or Enums). Localization is the responsibility of the frontend/UI layer:
  - `ene-desktop` translations reside under `apps/ene-desktop/i18n/{lang}/ene_desktop.ftl` as a single translation file.
  - `ene-cli` translations are managed locally within the CLI under `apps/ene-cli/i18n/`.
* **Visibility:** Default to `pub(crate)`. Only use `pub` when external consumers need it.
* **Comments:** Write `rustdoc` comments (`///`) for public APIs and complex logic. Re-exports should use `#[doc(no_inline)]`.

## 6. Common Tasks (Recipes)

### R1. Add a new tool
1. Create: `cargo new --bin tools/<name>`
2. Implement: `#[derive(ene_tool_derive::ToolSpec)]` on args structs.
3. Wire up: `run_tool_server::<MyAction>()` from `ene-tool-proto` in `main`.
4. Document: Add to a category in `docs/tools/` and `docs/ja/tools/`.
5. Verify: `cargo run -p ene-cli` -> `/tool list`.

### R2. Add a config field
1. Edit struct in `crates/ene-config/src/config.rs` (`define_config!` macro).
2. Run `cargo run -p ene-cli` once to auto-regenerate `assets/settings.schema.json` and `character_settings.schema.json`. *(Note: These JSON files are gitignored. Do not commit or hand-edit them).*
3. Document in `docs/configuration/settings.md` (both English and Japanese).

### R3. Add an IPC request/response
1. Extend `IpcRequest` / `IpcResponse` in `crates/ene-tool-proto/src/ipc.rs`.
2. Bump `PROTOCOL_VERSION` **only** if the wire format is incompatible.
3. Handle the variant in `ene-tool-host` and all tool binaries.

### R4. Add or modify localization (i18n) strings
1. **Desktop (`ene-desktop`)**:
    - Add or edit keys directly in the translation file located under `apps/ene-desktop/i18n/{lang}/ene_desktop.ftl`.
   - Use the `i18n_embed_fl::fl!(crate::i18n::loader(), "key-name")` macro to retrieve the localized string in your code.
2. **CLI (`ene-cli`)**:
   - Add or edit keys directly in the CLI's local Fluent files under `apps/ene-cli/i18n/{lang}/ene_cli.ftl`.
   - Use the `i18n_embed_fl::fl!(crate::i18n::loader(), "key-name")` macro inside the CLI code.

## 7. Configuration Data Flow

Loaded by `figment` in order: 1. Compile-time defaults -> 2. OS-standard User Config (or local `assets/settings.json`) -> 3. `ENE_` env vars.

## 8. Git & PR Policy

> **⚠️ Early Development Phase**
> The project is currently in the early development stage. **AI agents do not need to create branches or pull requests at this time.** Direct commits to `main` are acceptable while the project is still in this phase. The policies below (branch naming, PR checklist, etc.) will be enforced once the project transitions to a stable release milestone.

* **Branch Naming:** `<type>/<short-kebab-case>` (e.g., `feat/sandbox-gate`, `fix/cli-race`).
* **Commits:** Follow Conventional Commits (`feat: ...`, `fix: ...`).
* **Documentation:** English and Japanese (`docs/` and `docs/ja/`) must be kept in lock-step within the same PR.

### 8.1 Pre-commit Hooks (cargo-husky)

* `cargo-husky` is declared as a regular dep on `ene-core` and auto-installs hooks from `.cargo-husky/hooks/` into `.git/hooks/` on the first `cargo build`.
* **`pre-commit`** runs `cargo fmt --all` against staged `.rs` files and re-stages the changes. Skip per-commit with `git commit --no-verify`; skip all hooks with `HUSKY=0 cargo build`.

### PR Verification Checklist

Before submitting a PR or finishing a coding task, verify:

* [ ] `cargo fmt --all` and `cargo clippy --workspace -- -D warnings` are clean.
* [ ] `cargo test --workspace` passes.
* [ ] `sea-orm` migrations (Rust modules) are present, tested, and registered in the Migrator (if schema changed).
* [ ] Config-field changes do not require manual schema commits (auto-regenerated, gitignored).
* [ ] Public API or behavior changes have corresponding updates under `docs/`.
* [ ] Both English (`docs/`) and Japanese (`docs/ja/`) docs are updated for any user-visible change.
* [ ] Branch name and commit/PR title follow Conventional Commits.

## 9. Further Reading

See `docs/` for deep-dives into Architecture, Memory, Tools (RAG, SDK, Sandbox), and Core (Streaming, Prompting).
