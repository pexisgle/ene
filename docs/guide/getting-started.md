# Getting Started

Build and run ene locally, then point it at an LLM provider.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`
- On Linux, use the Nix/`direnv` workspace shell so `cargo` matches the project environment
- Native GUI deps if you build `ene-desktop` (GTK / Wayland / Windows SDK as needed)

## Build

Day-to-day development builds only `ene-cli` (the workspace `default-members` entry). Build other packages explicitly when you need them.

```bash
cargo build
cargo check -p ene-cli
cargo run -p ene-cli
```

Desktop:

```bash
cargo run -p ene-desktop
```

Full workspace (CI / pre-PR verification):

```bash
cargo build --workspace
cargo test --workspace
```

Release:

```bash
cargo build --workspace --release
```

### Build performance

The dev profile keeps workspace debug symbols minimal (`line-tables-only`), disables dependency debuginfo, and uses selective `opt-level = 2` for heavy runtime crates (`wgpu`, `egui`, `rapier3d`, etc.). `sccache` and `mold` (via the Nix shell) shorten rebuilds; sccache stores its cache under `~/.cache/sccache`, not in `target/`.

After changing profile settings, run `cargo clean` once before comparing `target/` size. Optional cleanup: `cargo install cargo-sweep` then `cargo sweep -t 7`.

Use `--profile debugging` when you need full debug symbols for stepping through code in a debugger.

## Run CLI

```bash
cargo run -p ene-cli -- --help
cargo run -p ene-cli
```

## Run Desktop

```bash
cargo run -p ene-desktop --release
```

Desktop loads config softly when needed, opens an `EneHandle`, and plays VRM from Performance cues.

## Configure AI providers

Settings load in order: compile-time defaults → OS user config (or local `assets/settings.json`) → `ENE_` environment variables.

Minimum useful knobs:

- `ai.providers.default` — `base_url` and `api_key` for your OpenAI-compatible API
- `ai.tasks.chat` — `provider`, `model`, and optional `max_tokens`
- `character` — folder name (e.g. `"Alicia"`) or card path
- `store.enabled` — turn long-term memory on/off

See [Configure](configure.md) for the short tour, and the [full settings reference](../reference/configuration/settings.md) for every field.

## What to read next

1. [System overview](system-overview.md) — crates and one turn
2. [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
3. [Tools catalog](tools/overview.md)
4. For contracts and APIs: [Reference](../reference/index.md)
