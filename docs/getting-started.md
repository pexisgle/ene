# Getting Started with Ene

This guide covers system prerequisites, setting up the development environment, building the workspace, and running the applications (`ene-cli` and `ene-desktop`).

---

## Prerequisites

Ene is built as a Rust 2024 workspace requiring the following system tools:

- **Rust toolchain**: 1.85+ (Rust 2024 edition support)
- **C/C++ Compiler**: `clang` / `gcc` and `cmake` (for `llama-cpp-2` and `libsqlite3-sys` bundling)
- **Graphics Libraries**: Vulkan / Wayland / X11 dev headers (for `ene-desktop` and `ene-vrm` via `wgpu`)
- **Audio Libraries**: `alsa` / `jack` dev headers (for `ene-voice` via `cpal`)

### Recommended Setup: Nix + direnv (Linux)

The repository provides a fully reproducible Nix flake setup:

```bash
# Allow direnv to load the checked-in environment
direnv allow

# Verify Rust and Cargo are ready
cargo --version
```

Alternatively, run commands inside the Nix environment directly:

```bash
nix develop --command cargo check
```

---

## Building the Workspace

Ene is composed of multiple workspace packages. Because `apps/ene-cli` is the default workspace member, standard `cargo` commands run against `ene-cli` unless `--workspace` or `-p <package>` is specified.

### Quick Compile Check

```bash
# Check default package (ene-cli)
cargo check

# Check all crates and tools in the workspace
cargo check --workspace
```

### Building Application Binaries

```bash
# Build the CLI REPL
cargo build -p ene-cli

# Build the Desktop GUI
cargo build -p ene-desktop

# Build all tool and provider plugins
cargo build --workspace --bins
```

---

## Running Applications

### 1. Ene CLI (`ene-cli`)

The CLI application provides an interactive REPL for chatting with Ene, inspecting memory, managing sessions, and testing tool plugins.

```bash
# Run CLI with default settings and built-in character card
cargo run -p ene-cli

# Run with custom character card or settings
cargo run -p ene-cli -- --character Alicia
```

#### Useful REPL Commands
- `/help` — Display available REPL slash commands.
- `/memory list` — View recalled memory facts for the active session.
- `/tool list` — List registered tool plugins and active MCP servers.
- `/session archive` — Archive current session and reset conversation context.

---

### 2. Ene Desktop (`ene-desktop`)

`ene-desktop` launches the GUI application featuring the animated 3D VRM avatar, speech synthesis, and real-time emotion/performance expressions.

```bash
# Run Desktop GUI
cargo run -p ene-desktop
```

---

## Workspace Validation

Before submitting changes, validate code formatting, lints, and test suites:

```bash
# 1. Format check
cargo fmt --all -- --check

# 2. Workspace lints
cargo clippy --workspace -- -D warnings

# 3. Workspace unit & integration tests
cargo test --workspace
```

---

## Next Steps

- Explore the overall [System Architecture](architecture.md).
- Read the [Configuration Guide](configuration.md) to set up LLM API keys (OpenAI, Anthropic, Ollama).
- Learn about the [Memory System](concepts/memory-system.md) and [IPC Plugin Architecture](concepts/plugins-and-mcp.md).
