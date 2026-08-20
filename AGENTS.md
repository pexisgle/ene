# AGENTS.md

This file provides guidance to opencode and Claude Code when working with code in this repository.

## Release status

This app is **unreleased** and has no external consumers, so backward compatibility is not a
constraint. Don't preserve config schemas, IPC/plugin protocols, persisted data, or CLI surfaces
for compatibility's sake; prefer clean changes over legacy shims.

## Build environment

Linux native dependencies (Vulkan, ALSA, OpenSSL, libclang, mold) come from the checked-in
Nix flake. Run Linux builds from the repo root. If `direnv` is active, plain `cargo` works;
otherwise prefix with `nix develop --command`. GitHub Actions Linux jobs install the same
native deps with apt (`.github/actions/rust-build-env`) rather than entering the Nix shell.

**Platform support**: Linux and native Windows are supported development platforms. Linux CI
runs on `ubuntu-latest`; native Windows CI runs on `windows-latest`. Windows development uses
the stable MSVC Rust toolchain, the Visual Studio C++ workload, and the Windows SDK. The
flake's `x86_64-pc-windows-gnu` cross target remains available for Linux-side Windows builds.
macOS is not a supported target.

## Commands

`default-members = ["apps/ene-ctl"]`, so **a bare `cargo test` / `cargo clippy` only covers
the CLI client, not the workspace.** Always pass `--workspace` or `-p <package>` explicitly.

| Purpose | Command |
|---|---|
| Format | `cargo fmt --all` (check: `-- --check`) |
| Focused iteration | `cargo check -p <pkg>` / `cargo test -p <pkg>` |
| Full lint (CI gate) | `cargo clippy --workspace --all-targets -- -D warnings` |
| Full tests (CI gate) | `cargo test --workspace` |
| Run | `cargo run -p ene-ctl -- --help` / `cargo run -p ene-stage` / `cargo run -p ene-daemon` |

CI additionally runs `cargo doc --workspace --no-deps`. It is deliberately *not* run with
`RUSTDOCFLAGS=-D warnings` — pre-existing broken intra-doc links would fail unrelated work.

## Lints are the spec, not a suggestion

`[workspace.lints.clippy]` in the root `Cargo.toml` denies `all`, `pedantic`, and `cargo` as
whole groups, plus these individually: `unwrap_used`, `expect_used`, `panic`, `todo`,
`unimplemented`, `dbg_macro`, `mem_forget`, `let_underscore_must_use`, `print_stdout`,
`print_stderr`, `allow_attributes`, `allow_attributes_without_reason`. Clippy failures are
build failures.

- No `unwrap`/`expect`/panic paths in production code. Tests opt out per-crate via
  `#![cfg_attr(test, expect(clippy::unwrap_used, ...))]` — see `crates/ene-session/src/lib.rs`.
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
  crate level only for `apps/ene-ctl`, plugin binaries' fatal-error paths, and examples.
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
  sound (see `crates/ene-sandbox/src/linux.rs`).
- Commented-out code is deleted, not parked.
- Comments and rustdoc are **English only**, regardless of the working language — the
  bilingual rule applies to `docs/` and i18n strings, not to source.

## Architecture boundaries

Violating these is the most common way to break this repo:

- `ene-session` owns the append-only conversation log and usage ledger. Other crates
  read history through its projection APIs; they do not open `sessions.db` themselves.
- `ene-kernel` owns the dialogue lane (`prompt` / `steer` / `follow_up` / `abort` /
  `compact`). It must not depend on HTTP, plugins, or companion persistence.
- `ene-companion` owns soul, affect, memory, inner channel, proactive speech, and
  character packages. It must not depend on the daemon or the tool host.
- `ene-work` owns delegation, jobs, schedules, skills, and MCP bindings. Mutating work
  is gated on plan approval in `ene-plane`, not on mailbox text.
- `ene-plane` owns approval, the audit hash chain, and the credential vault.
- `ene-fiber` owns plugin process supervision and reversible host-context composition.
  Child kill is not unload; host registrations must be disposed LIFO.
- `ene-plugin-ipc` is wire ABI only (split `core` / `tool` subprotocols). `ene-registry`
  is the unified tool pipeline. Never move business or DB logic into the ABI crate.
- `ene-card` owns Character Card V3 / PNG / CHARX import. It depends only on
  `ene-config`; `ene-config` must never depend on it, or the `zip` dependency leaks
  back into the settings core.
- `ene-vrm` is rendering-only. It must not depend on kernel, companion, or work.
- `ene-daemon` (`apps/ene-core`, binary `ene-core`) is the process that wires the
  libraries and serves HTTP/WS. Clients (`ene-ctl`, `ene-stage`, Web) talk only through
  `ene-api`.

## Docs and where truth lives

**rustdoc is authoritative for all signatures** (`cargo doc -p <crate> --open`).
`docs/reference/crates.md` intentionally contains no hand-written signatures —
it covers role, boundaries, and rationale only. Do not transcribe signatures
into Markdown.

Orientation: `docs/index.md`, `docs/quickstart.md`, `docs/configuration.md`,
`docs/concepts/`, `docs/apps/`, `docs/guides/`, `docs/reference/`.
User-facing docs are bilingual: every change under `docs/` needs the matching file under
`docs/ja/`. Backend event and status names stay stable English contracts.

## Configuration

Precedence is defaults → JSON → `ENE_` env vars, with `__` separating nested keys
(e.g. `ENE_CORE__SERVER__BIND`). Add settings at the owning `define_config!` invocation,
which often lives outside `ene-config` (`ene-session`, `ene-kernel`, `ene-companion`,
`ene-body`, `ene-plane`). Schemas regenerate automatically at config init —
`assets/schema/*` is gitignored; never hand-edit or commit it.

## Plugins and IPC

New tools are separate lightweight binaries: `cargo new --bin plugins/harness/<name>`.
Serve via `ene_registry::run_plugin(BuiltinKind::…)` (see `plugins/harness/fs/src/main.rs`).
Use namespaced `<namespace>.<action>` names and declare side effects. Verify through
`ene-ctl` and update both `docs/concepts/plugins-and-mcp.md` and its `docs/ja/` counterpart.

Plugin crates are **binary-only** — no `[lib]` target (see `plugins/harness/fs`). Size is
not a reason to add one: `#[cfg(test)]` modules run normally in a bin crate. Add `[lib]`
only when an integration test under `tests/` or another workspace crate must link the
logic directly.

One plugin per native runtime. `llama.cpp`, `whisper.cpp`, and ONNX Runtime each get their
own plugin binary — never bundle two native runtimes into one.

Provider plugins: add a binary under `plugins/provider/` **and** a row in
`ene_fiber::PROVIDER_PLUGINS` (id, seams, `local`, `needs_key`). Desktop pickers
read `GET /api/v1/settings` → `effective.providers` from that table — do not add
a parallel UI allowlist. Each `ai.tasks.*` binding gets its own fiber so chat and
embedding can share a plugin with different GGUFs. Local GGUF is
`plugins/provider/gguf` (`provider.gguf`). Embeddings are optional (`seam.embed`
plugins, or unset).

Sidecar pattern: provider plugins that run a local engine as a child process follow
`templates/sidecar` — the host spawns the sidecar on a loopback port via
`ene-fiber` (`broker.spawn_sidecar`), health-polls with a timeout, and kills on
fiber unload. **Sidecar engines** (`llama-server`, VOICEVOX Engine) use a
**host-managed runtime catalog**: `ene-provider-assets` fetches GitHub Releases
at startup (6h TTL disk cache under `data_dir/catalog-cache/`), installs
verified artifacts under `data_dir/plugins/<plugin_id>/assets/`, and exposes
`list` / `install` / `install_status` / `set_active` via `ene-fiber` and
`ene-core` HTTP (`refresh_catalog` forces a refetch). Install keys are
`{release_tag}/{variant_id}`. **GGUF weights** stay in each plugin's static
catalog; the host merges probe `assets.list` with runtime sidecar rows. Plugins
must never download from arbitrary URLs — only host-allowlisted GitHub and
Hugging Face prefixes.

IPC lives in `crates/ene-plugin-ipc` (split `core` / `tool` subprotocols, length-prefixed
MessagePack frames). `id` is required on every request/response. Prefer adding
`#[serde(default)]` fields over bumping a subprotocol version.

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
- **Stage (`ene-stage`)** runs headless via **software Vulkan (lavapipe)**: launch with
  `DISPLAY=:1 WGPU_BACKEND=vulkan`; it also needs `libxkbcommon-x11`.
- **Chat requires a configured model.** Bind `ai.tasks.chat` from the AI page
  (installed provider plugins from the host catalog, including local GGUF on
  `provider.gguf`) before sending a message. Embeddings are a separate optional
  task.
