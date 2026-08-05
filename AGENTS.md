# AGENTS.md

This file provides guidance to opencode and Claude Code when working with code in this repository.

## Build environment

Native deps (Vulkan, ALSA, OpenSSL, libclang, mold) come from the checked-in Nix flake.
Run everything from the repo root. If `direnv` is active, plain `cargo` works; otherwise
prefix with `nix develop --command` (this is what CI does).

**Platform support**: Linux is the only supported dev/CI platform (all CI jobs run on
`ubuntu-latest`). Windows is produced by cross-compiling from Linux to the
`x86_64-pc-windows-gnu` target via the flake's mingw toolchain — there is no native Windows
dev shell. macOS is not a supported target.

## Commands

`default-members = ["apps/ene-cli"]`, so **a bare `cargo test` / `cargo clippy` only covers
the CLI, not the workspace.** Always pass `--workspace` or `-p <package>` explicitly.

| Purpose | Command |
|---|---|
| Format | `cargo fmt --all` (check: `-- --check`) |
| Focused iteration | `cargo check -p <pkg>` / `cargo test -p <pkg>` |
| Full lint (CI gate) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Full tests (CI gate) | `cargo test --workspace` |
| Run | `cargo run -p ene-cli -- --help` / `cargo run -p ene-desktop` |

CI additionally runs `cargo doc --workspace --no-deps`. It is deliberately *not* run with
`RUSTDOCFLAGS=-D warnings` — pre-existing broken intra-doc links would fail unrelated work.

## Lints are the spec, not a suggestion

`[workspace.lints.clippy]` in the root `Cargo.toml` denies `all`, `pedantic`, and `cargo` as
whole groups, plus these individually: `unwrap_used`, `expect_used`, `panic`, `todo`,
`unimplemented`, `dbg_macro`, `mem_forget`, `let_underscore_must_use`, `print_stdout`,
`print_stderr`, `allow_attributes`, `allow_attributes_without_reason`. Clippy failures are
build failures.

- No `unwrap`/`expect`/panic paths in production code. Tests opt out per-crate via
  `#![cfg_attr(test, expect(clippy::unwrap_used, ...))]` — see `crates/ene-runtime/src/lib.rs`.
- `allow_attributes` and `allow_attributes_without_reason` are denied: every exception must be
  `#[expect(lint, reason = "...")]`, scoped as narrowly as possible — `#[allow]` is rejected
  outright, so a stale exception surfaces as `unfulfilled_lint_expectations` instead of
  silently lingering. Never widen a lint workspace-wide to make an error go away.
- `clippy::restriction` is intentionally *not* blanket-enabled; adopt lints from it one at a
  time with a reason comment.

## Rust conventions (review-enforced, not caught by clippy)

- Async is Tokio. Library errors are `thiserror` — never expose `anyhow`, bare `String`, or
  `Box<dyn Error>` at a library boundary.
- Diagnostics use structured `tracing`. `print_stdout`/`print_stderr` are re-allowed at the
  crate level only for `apps/ene-cli`, plugin binaries' fatal-error paths, and examples.
- Prefer `parking_lot` locks, `OnceLock` for one-time init, and the narrowest visibility
  (`pub(crate)` by default).
- Keep deps in root `[workspace.dependencies]` and reference them as `{ workspace = true }`.
  Don't bump versions as a side effect of unrelated work.

## Comments

The default is **no comment**. A comment earns its place only by carrying information the
code cannot express: why this way, what breaks otherwise, where a value came from. Rustdoc
on public items is the exception — it states the contract, not the mechanism.

- **Never restate the code.** If a comment paraphrases the line below it, delete it and fix
  the naming instead. Aim for the smallest number of comments that keeps the code
  maintainable, not for coverage.
- **Write the *why*.** Worth a comment: invariants a caller must uphold, non-obvious
  ordering/locking requirements, why the obvious simpler version fails, the origin of a
  magic value (spec, protocol version, upstream bug, measured limit).
- **Self-contained — no own-repo references.** A comment must be fully understandable from
  the repo alone. No issue/PR numbers (`(#240)`, `TODO(#218)`), no links to this repo's
  GitHub/Slack/CI, no "see the review thread". If the context lives in a ticket, summarize
  its substance in one line; if it isn't worth that line, it isn't worth a comment. Existing
  `(#nnn)` markers are legacy — drop or rewrite them when you touch the line. The same ban
  applies to logs, user-facing strings, and assertion / `expect` messages — not only
  comments and rustdoc.
- **External specs are the exception.** Links to the standards a value or format comes from
  (VRM spec, Character Card Spec, glTF, library docs) document provenance the code cannot
  express — keep them. Only *this repo's* issue/PR/CI references are banned.
- **When in doubt, leave it out.** Re-read every comment against these rules: one that
  restates code or adds colour is deleted, not kept. The default answer to "should this
  comment exist?" is no.
- **No changelog in comments.** "Moved from `ene-store`", "previously used X", "changed in
  v2" describe a diff, not the code. `git log` and the generated changelog own that.
- `TODO:` states *what* and *under which condition*, never a ticket:
  `// TODO: drop the fallback once every plugin negotiates IPC v4`.
- `// SAFETY:` is mandatory on every `unsafe` block and must name the invariant that makes it
  sound (see `plugins/provider/local-llm/src/local_llm/model.rs`).
- Commented-out code is deleted, not parked.
- Comments and rustdoc are **English only**, regardless of the working language — the
  bilingual rule applies to `docs/` and i18n strings, not to source.

## Architecture boundaries

Violating these is the most common way to break this repo:

- `ene-runtime` — host/actor facade and bootstrap. `ene-mind` owns session, recall, prompt
  composition, affect, performance, and memory writing, and **must not depend on runtime or
  the tool host**.
- `ene-store` alone owns SQLite/SeaORM connections, schema, migrations, and raw DB access. It
  must not depend on ai/mind. Plugin binaries reach state through `ene-plugin-db` over IPC.
- `ene-plugin-proto` is wire ABI only. `ene-plugin` is the authoring facade,
  `ene-plugin-host` owns process/registry orchestration, `ene-vrm` is rendering-only.
  Never move business or DB logic into ABI crates.
- `ene-util` — dependency-isolated pure utility functions (truncate, html). No I/O, no
  business logic, no state. New helpers go here only if they are pure functions with
  dependencies that can be feature-gated; anything else belongs in a domain crate.
- API v1 invariants: every turn has a `TurnId`; `run` is single-flight and returns
  `RunError::Busy`; `Terminal` follows history commit and synchronous finalization (deferred
  memory work may continue after it); `Performance` is the presentation event, kept separate
  from detailed pipeline diagnostics.
- `release` deliberately omits `panic = "abort"` — `ene-runtime`'s actor relies on
  `catch_unwind` isolation (`crates/ene-runtime/src/handle/actor.rs`). Do not add it.

## Docs and where truth lives

**rustdoc is authoritative for all signatures** (`cargo doc -p <crate> --open`).
`docs/reference/crates.md` intentionally contains no hand-written signatures —
it covers role, boundaries, and rationale only. Do not transcribe signatures
into Markdown.

Orientation: `docs/index.md`, `docs/quickstart.md`, `docs/configuration.md`,
`docs/concepts/`, `docs/apps/`, `docs/guides/`, `docs/reference/`.
User-facing docs are bilingual: every change under `docs/` needs the matching file under
`docs/ja/`. UI strings live in `apps/ene-desktop/i18n/{en-US,ja}/` and
`apps/ene-cli/i18n/{en-US,ja}/` — keep both locales in sync. Backend event and status names
stay stable English contracts.

## Configuration

Precedence is defaults → JSON → `ENE_` env vars, with `__` separating nested keys
(e.g. `ENE_AI__TASKS__CHAT__MODEL`). Public sections: `ai.*`, `store.*`, `mind.*`,
`plugins.*`, `desktop.*`; plugin entries are `plugins.list.<name>` with flattened fields.

Add settings at the owning `define_config!` invocation, which often lives outside
`ene-config` (`ene-ai`, `ene-mind`, `ene-store`, `ene-plugin-host`, `apps/ene-desktop`).
Schemas regenerate automatically at config init — `assets/schema/*` is gitignored; never
hand-edit or commit it.

## Plugins and IPC

New tools are separate lightweight binaries: `cargo new --bin plugins/tool/<name>`. Derive
`ToolAction`, build on `ene_plugin::{prelude, ActionSetProvider}`, then serve via
`run_plugin_server(PluginDispatch::new(Some(Arc::new(ToolProviderPlugin::new(provider))), None, None, None, None))`
(see `plugins/tool/utility/src/main.rs`). Use namespaced `<namespace>.<action>` names and
declare side effects / sandbox needs. Verify with `/tool list` and update both
`docs/concepts/plugins-and-mcp.md` and its `docs/ja/` counterpart.

Plugin crates are **binary-only** — no `[lib]` target (see `plugins/tool/fs`,
`plugins/provider/anthropic`). Size is not a reason to add one: `#[cfg(test)]` modules run
normally in a bin crate. Add `[lib]` only when an integration test under `tests/` or another
workspace crate must link the logic directly.

One plugin per native runtime. `llama.cpp`, `whisper.cpp`, and ONNX Runtime each get their
own plugin binary — never bundle two native runtimes into one. Their build characteristics
differ sharply (ONNX Runtime is `load-dynamic` and needs no compilation; llama.cpp and
whisper.cpp are cmake C++ builds that also cross-compile to mingw), so bundling forces every
user of one onto the build cost of the other.

IPC starts at `crates/ene-plugin-proto/src/ipc.rs` (protocol v7, length-prefixed frames). The
host advertises a range via `VersionRange::host_supported()` and keeps N-1 compatibility, so
`PLUGIN_IPC_MIN_SUPPORTED_VERSION = PLUGIN_IPC_PROTOCOL_VERSION - 1`. Prefer adding
`#[serde(default)]` fields over bumping the version; gate behavior on newer messages via
`IpcPluginConnection::negotiated_version()` (see `supports_cancel_stream()` for the pattern).

## Repo etiquette

- Conventional Commits — `cliff.toml` generates the changelog from them.
- `cargo-husky` installs a pre-commit hook that runs `cargo fmt --all` (whole tree, not just
  staged files) and re-stages formatted files. Inspect the resulting diff. Never use
  `--no-verify` to get past a failing check.
- Never commit or log secrets, `.env`, `memory.db*`, `undo.db*`, `todo.db*`, model weights,
  or anything under `assets/models/`.
- Report the failing package and root cause rather than relaxing a lint. Don't claim a check
  passed that you didn't run.

## Cursor Cloud specific instructions

The Cloud Agent VM has **no Nix and no direnv**, so the "Build environment" flow above does not
apply as written. `cargo` is on `PATH` via a rustup `stable` toolchain — run `cargo <cmd>`
directly from the repo root (do not use `direnv exec` / `nix develop`, and ignore the
`rtk cargo` user rule; those tools are not installed). Native build deps come from the system
image (apt) instead of the flake and are baked into the VM; the startup script only runs
`cargo fetch`.

- **Toolchain:** edition 2024 needs Rust ≥ 1.85; the VM uses rustup `stable` (the old system
  `rustc` 1.83 is too old — don't pin to it).
- **C/C++ toolchain (do not revert):** `cc`/`c++` are pinned to **GCC** via
  `update-alternatives`. Clang 18 here cannot find libstdc++ headers (`cstdint`) or `-lstdc++`,
  which breaks native C/C++ build deps and final linking. If a build fails with those errors,
  run `sudo update-alternatives --set cc /usr/bin/gcc && sudo update-alternatives --set c++ /usr/bin/g++`.
- **Don't set `RUSTFLAGS`/`CC`/`CXX` for normal builds** — changing them invalidates the cache
  and forces a full (~10+ min) workspace rebuild.
- **Desktop (`ene-desktop`)** runs headless via **software Vulkan (lavapipe)**: launch with
  `DISPLAY=:1 WGPU_BACKEND=vulkan`; it also needs `libxkbcommon-x11`.
- **Live chat needs credentials.** The bundled `assets/settings.json` ships without a usable
  chat key, so out-of-the-box chat fails; provide credentials via the `ai.*` config
  (`ENE_AI__…` env overrides, see the Configuration section). Embeddings/local models are
  fetched to `assets/models/` (gitignored) on first use.
