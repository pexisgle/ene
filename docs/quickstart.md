# Quickstart

This page gets you from a fresh checkout to a running daemon and client.
Build-environment details live in the repository's `AGENTS.md`.

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
cargo build --workspace
cargo build -p ene-ctl
```

## 3. Run the CLI

```sh
# Start the core daemon, then talk to it
cargo run -p ene-ctl -- core start
cargo run -p ene-ctl -- --help
```

`ene-ctl` uses the same HTTP/WS API as desktop, stage, and Web. Point `--url`
and `--token` (or `ENE_API_URL` / `ENE_API_TOKEN`) at an already-running
`ene-core` if you started it yourself.

## 4. Run the desktop client

```sh
cargo run -p ene-desktop
```

Desktop starts `ene-core` as a child when needed, shows the character overlay
and chat on the surface depth, and opens a separate detail window (F4 / tray)
for logs and internals. `ene-stage` is an optional debug client for the same
API.

Without a conversation provider plugin, replies are Echo-only.

## 5. Tests

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Always pass `--workspace` or `-p <package>`: default-members is `ene-ctl` only.
