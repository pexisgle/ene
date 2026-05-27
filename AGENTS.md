# AGENTS.md — ene

## Environment Setup
- **Linux**: Use `direnv + flake`. Run `direnv allow` in the root. The flake provides Rust nightly, OpenSSL, mold, clang, GTK3, Wayland, Chromium, vulkan, and all native deps.
- **Non-interactive shell**: Use `direnv exec . <command>` to run commands inside the flake environment without sourcing `.envrc` (e.g., `direnv exec . cargo build --workspace`).
- **`.envrc`**: Runs `use flake` then loads `.env` for API tokens.
- **`.env`**: Copy from `.env.example`, set `API_TOKEN` for OpenAI-compatible LLM access.
- **Toolchain**: `nightly` (see `rust-toolchain.toml`).

## Key Commands
```bash
cargo build --workspace          # Full build
cargo build --workspace --release
cargo run -p ene-desktop --release   # GUI app
cargo run -p ene-cli --              # CLI REPL
cargo test --workspace               # All tests
cargo clippy --workspace             # Lint
```

## Workspace Structure
```
crates/
  ene-core        — Unified runtime facade, run_ai_with_tools(), streaming engine
  ene-config      — JSON settings, figment, define_config! macro
  ene-embedding   — Vector embeddings (API + local GGUF/candle)
  ene-memory      — SQLite-vec long-term memory store
  ene-session     — Conversation history, CharacterCardV3, auto-split
  ene-tool-proto  — IPC protocol, ToolProvider trait
  ene-tool-host   — Tool process manager, MCP support, Tool RAG
  ene-tools/      — IPC tool binaries (fs, web, utility, app, browser)
apps/
  ene-desktop     — Bevy GUI (VRM character, always-on-top overlay, egui settings)
  ene-cli         — tokio::main REPL with /commands
```

## Architecture Notes
- **Tool execution**: Tools run as separate binaries via IPC (Unix Domain Sockets / Windows Named Pipes). `ene-tool-host` manages lifecycle with crash resilience (exponential backoff, max 5 restarts).
- **Data flow**: User Input → AiRuntime → Memory Search → build_messages() → LLM stream → TextDelta / ToolCall / Finished events.
- **Session splitting**: Automatic based on timeouts and topic drift. Summaries stored in memory.
- **Emotion tokens**: `<|emo:name|>` syntax parsed from LLM output, mapped to VRM blendshapes with 4s hold + fade out.

## Platform-Specific Gotchas
- **Linux linker**: `.cargo/config.toml` sets `linker = "clang"` and `mold` for x86_64-unknown-linux-gnu.
- **GUI native deps**: GTK3, Wayland, alsa-lib, mesa, vulkan-loader, pipewire, xdotool (enigo), libayatana-appindicator. All provided by flake on Linux.
- **Desktop window**: Always-on-top, transparent, borderless fullscreen on Linux.
- **Release profile**: `codegen-units = 1`, `lto = "fat"`, `opt-level = "z"`, `strip = true`, `panic = "abort"`.
- **Dev profile**: `opt-level = 1` globally, `opt-level = 3` for dependencies.

## Configuration
- Settings loaded from JSON via `figment`. Schema auto-generated as `settings.schema.json`.
- Character cards: `CharacterCardV3` format, loaded from CLI args or auto-discovered.
- Resource dirs created on first run via `ensure_resource_dirs()`.

## Testing
- `cargo test --workspace` for all tests.
- CLI has `--tooltest` flag for one-shot tool testing.
- REPL `/tooltest [prompt]` for interactive tool testing.

## Docs
- Full documentation in `docs/`. Key files:
  - `docs/architecture/overview.md` — Crate map and dependency graph
  - `docs/architecture/startup.md` — Boot sequences for desktop and CLI
  - `docs/configuration/settings.md` — settings.json schema
  - `docs/tools/` — Tool system documentation
  - `docs/core/` — Streaming, prompt, session, emotion docs
