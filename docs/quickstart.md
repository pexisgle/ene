# Quickstart

This page gets you from a fresh checkout to a running chat. The full
build-environment details live in the repository's `AGENTS.md`; this page is
the short version.

## 1. Requirements

- **Linux** is the only supported development and CI platform. Windows is
  cross-compiled from Linux; macOS is not supported.
- **Rust ≥ 1.85** (the workspace uses edition 2024). CI uses the stable
  toolchain.
- Native dependencies: Vulkan, ALSA, OpenSSL, `libclang`, `mold`, and
  Wayland/X11 development packages. The checked-in Nix flake provides all of
  them:

```sh
nix develop --command cargo build --workspace
```

If `direnv` is active in the repository, plain `cargo` works directly.

## 2. Build

```sh
# Everything (apps, crates, plugins)
cargo build --workspace

# Just the CLI (fastest iteration)
cargo build -p ene-cli
```

Release builds (`--release`) are supported; the release profile deliberately
keeps `panic = "unwind"` because the runtime's fault-tolerance depends on it
(see [Architecture](concepts/architecture.md#fault-tolerance)).

## 3. Run the CLI

```sh
cargo run -p ene-cli
```

With no arguments the CLI starts an interactive REPL. Type `/help` to list
all slash commands, or just type a message to talk to the configured
character.

Useful first checks:

```sh
# Environment health check (config, providers, store, plugins)
cargo run -p ene-cli -- doctor

# One-shot prompt (non-interactive, exits afterwards)
cargo run -p ene-cli -- run "Hello!"
```

### First-run behavior

On first launch Ene deploys its bundled assets (sample characters, prompt
packs) into the assets directory and writes a default `settings.json`. In
debug builds the repository's `assets/` folder is used directly; release
builds use the OS application-data directory (`~/.local/share/ene` on
Linux, `%APPDATA%\ene` on Windows). See [Configuration](configuration.md)
for details.

## 4. Configure a chat provider

A chat turn needs an LLM provider. The bundled default card is `Alicia` and
the default `settings.json` is configured for the cloud provider
`openrouter`, but ships **without an API key**. Either add a key or switch
to a local model.

### Cloud provider (API key via environment)

```sh
export OPENROUTER_API_KEY="sk-..."
cargo run -p ene-cli
```

Any settings value can be overridden with `ENE_`-prefixed environment
variables using `__` for nesting — for example, to change the chat model:

```sh
ENE_AI__TASKS__CHAT__MODEL="openai/gpt-5.6-luna" cargo run -p ene-cli
```

### Fully local (no network)

The `local-llm` provider plugin runs GGUF models through llama.cpp on your
machine. Edit `settings.json` (`ai.tasks.*.provider = "local"`,
`ai.local_models.<name>` entries) or use the environment overrides; the
plugin downloads model files from Hugging Face on first use:

```sh
ENE_AI__TASKS__CHAT__PROVIDER="local" \
ENE_AI__TASKS__CHAT__MODEL="gemma-4-e2b" \
cargo run -p ene-cli
```

See [Configuration → AI](configuration.md#ai) and
[Concepts → Plugins](concepts/plugins-and-mcp.md) for the full provider
setup, including TTS/STT and embeddings.

## 5. Run the desktop app

```sh
cargo run -p ene-desktop
```

The desktop app opens a window with the 3D VRM avatar and a chat pane. Two
optional positional arguments select a VRM model and a VRMA motion clip.
See the [Desktop guide](apps/desktop.md) for features and platform notes.

## 6. Verify your setup

| Check | Command |
|---|---|
| Health of config, providers, store, plugins | `ene doctor` (REPL: `/doctor`) |
| Registered tools | `ene tool list` (REPL: `/tool list`) |
| Loaded character | `ene characters` |
| Memory store | `ene memory list` |
| Tests | `cargo test --workspace` |
| Lints (CI gate) | `cargo clippy --workspace --all-targets -- -D warnings` |

## What's next

- Understand how a turn flows through the system: [Architecture](concepts/architecture.md)
- Tune settings: [Configuration](configuration.md)
- Add your own character: [Character cards](concepts/character-cards.md)
- Give the character new abilities: [Write a tool](guides/tools/write-a-tool.md)
