# Getting Started

Build and run ene locally, then point it at an LLM provider.

## Prerequisites

- Rust toolchain from `rust-toolchain.toml`
- On Linux, use the Nix/`direnv` workspace shell so `cargo` matches the project environment
- Native GUI deps if you build `ene-desktop` (GTK / Wayland / Windows SDK as needed)

## Build

```bash
cargo build --workspace
```

Release:

```bash
cargo build --workspace --release
```

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

## Configure a provider

Settings load in order: compile-time defaults → OS user config (or local `assets/settings.json`) → `ENE_` environment variables.

Minimum useful knobs:

- `provider.base_url` / `provider.model` / `provider.api_key`
- `character` — card name or path
- `store.enabled` — turn long-term memory on/off

See [Configure](configure.md) for the short tour, and the [full settings reference](../reference/configuration/settings.md) for every field.

## What to read next

1. [System overview](system-overview.md) — crates and one turn
2. [CLI](apps/cli.md) / [Desktop](apps/desktop.md)
3. [Tools catalog](tools/overview.md)
4. For contracts and APIs: [Reference](../reference/index.md)
