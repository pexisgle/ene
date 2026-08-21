# Quickstart

This page gets you from a fresh checkout to a running daemon and client.
Build-environment details live in the repository's `AGENTS.md`.

## 1. Requirements

- **Linux and native Windows** are supported development platforms. macOS is
  not supported.
- **Rust ≥ 1.85** (the workspace uses edition 2024). Use the stable toolchain.
- On Linux, native dependencies are Vulkan, ALSA, OpenSSL, `libclang`, `mold`,
  and Wayland/X11 development packages. The checked-in Nix flake provides them.
- On Windows, install the stable MSVC Rust toolchain, Visual Studio 2022
  Build Tools with **Desktop development with C++**, and the Windows 10/11
  SDK. The desktop uses DX12 and WASAPI on Windows.

```sh
nix develop --command cargo build --workspace
```

If `direnv` is active in the repository, plain `cargo` works directly.

## 2. Build

Linux builds can use the workspace commands directly:

```sh
cargo build --workspace
cargo build -p ene-ctl
```

For native Windows desktop development, build the daemon alongside the client
so `ene-stage` can start `ene-core` from `target/debug`:

```powershell
cargo build -p ene-daemon -p ene-stage
```

## 3. Run the CLI

```sh
# Start the core daemon, then talk to it
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- --help
```

`ene-ctl` uses the same HTTP/WS API as stage and Web. Point `--url`
and `--token` (or `ENE_API_URL` / `ENE_API_TOKEN`) at an already-running
`ene-core` if you started it yourself.

## 4. Run the product GUI (stage)

```sh
cargo run -p ene-stage
```

On native Windows, run the same command from PowerShell after building both
`ene-daemon` and `ene-stage` as shown above. Set `ENE_CORE_BIN` if the daemon
binary is stored outside `target/debug`.

Stage starts `ene-core` as a child when needed, shows the character overlay
and chat on the surface depth, and opens a separate detail window (F1 / tray)
for settings, memory, character, jobs, and internals.

Chat has no default model — pick one in the detail **Conversation** tab before
your first message. Bind an **installed provider plugin** from the host catalog
(`seam.llm`): `provider.gguf` (This computer, local GGUF), OpenAI-compatible,
Anthropic, and any plugin you add. Download a recommended Gemma GGUF or pick
your own `.gguf` on the local plugin; cloud plugins take a model name and a
vault API key.

Embeddings are optional: unset, local GGUF (`provider.gguf`, recommended Jina),
or a cloud embed plugin. Empty classifier and proactive tasks inherit chat.

## 5. Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Always pass `--workspace` or `-p <package>`: default-members is `ene-ctl` only.

The repository `assets/settings.json` is a development sample. `ene-core`
reads `settings.json` from the data directory (defaults apply when a key is
omitted).
