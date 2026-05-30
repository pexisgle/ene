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
  ene-common       — Common types and utilities
  ene-core         — Unified runtime facade, EneHandle/EneActor, streaming engine, run_ai_with_tools()
  ene-config       — JSON settings via figment, define_config! macro
  ene-embedding    — Vector embeddings (API + local GGUF/candle)
  ene-memory       — SQLite-vec long-term memory store (summaries, key facts, tool embeddings)
  ene-provider     — LLM provider abstraction
  ene-session      — Conversation history, CharacterCardV3, auto-split
  ene-tool-proto   — IPC protocol, ToolProvider trait, IpcRequest/IpcResponse wire format
  ene-tool-host    — Tool process manager, MCP support, Tool RAG, CompositeToolRegistry
  ene-tools/
    common/        — Shared tool utilities (SandboxConfigData, etc.)
    fs/            — Filesystem tools (read/write/edit/delete/glob/grep/patch, shell, undo)
    web/           — Web tools (webfetch, websearch)
    utility/       — Utility tools (question, todo, get_current_time, get_system_info)
    app/           — GUI automation (window mgmt, input, screenshot, clipboard)
    browser/       — Browser automation (Chromium CDP via chromiumoxide)
apps/
  ene-desktop      — Bevy GUI (VRM character, always-on-top overlay, egui settings)
  ene-cli          — tokio::main REPL with /commands
```

## Architecture Notes
- **Actor-based architecture**: `EneHandle` (public API) → mpsc `EneCommand` → `EneActor` (tokio) → broadcast `EneEvent`. `EneHandle::Drop` sends Shutdown when last handle.
- **EneCommand**: Run, Cancel, Shutdown, Reconfigure, LoadCharacter, PermissionDecision, GetSnapshot, ManualSplit (pub(crate), not exported)
- **EneEvent**: TextDelta, SpecialToken, ToolCallStart, ToolCallResult, PermissionRequired, TaskProgress, SessionSplit, Done, Failed, StatusChanged
- **Data flow**: User Input → EneCommand::Run → EneActor → Memory Search → build_messages() → LLM stream → EneEvent pipeline
- **Tool execution**: Tools run as separate binaries via IPC (Unix Domain Sockets / Windows Named Pipes). `ene-tool-host` manages lifecycle with crash resilience (exponential backoff, max 5 restarts). Binary discovery: `builtin_tools_dir()` (debug: same dir, release: `exe_dir/tools/`), `user_tools_dir()` (`app_data_dir()/tools/`).
- **IPC Protocol**: `IpcRequest` (Initialize, ListTools, CallTool, SetSessionId, Ping, Shutdown) ↔ `IpcResponse` (Ack, Tools, CallResult, Pong, Error). Wire format: 4-byte big-endian length prefix + JSON payload.
- **Tool Registry**: `ToolRegistry` trait → `CompositeToolRegistry` (first-wins dedup, Tool RAG embedding, cosine-similarity filtering). Also supports MCP via `McpToolRegistry`.
- **Session splitting**: Automatic based on timeouts (`session_timeout_minutes`) and topic drift (cosine similarity < `topic_change_threshold`). Summaries stored in memory. Manual split via `/session split`.
- **Emotion tokens**: `<|emo:name|>` syntax parsed from LLM output via `split_text_and_special_tokens()` and `extract_emotion_from_token()`. Desktop: 4s hold + fade out → VRM blendshape. CLI: `[Emotion: name]` in magenta.
- **Prompt construction**: `build_messages()` assembles: system prompt → example messages (first turn) → recalled summaries → key facts → conversation history → expression protocol → current input. Supports CBS macros (`{{char}}`, `{{user}}`, `{{random:...}}`, etc.).

## Platform-Specific Gotchas
- **Linux linker**: `.cargo/config.toml` sets `linker = "clang"` and `mold` for x86_64-unknown-linux-gnu. Also enables `--gc-sections` and `--icf=all` optimizations.
- **Dev codegen**: Uses `cranelift` backend for faster compilation (deps still use `llvm`).
- **GUI native deps**: GTK3, Wayland, alsa-lib, mesa, vulkan-loader, pipewire, xdotool (enigo), libayatana-appindicator. All provided by flake on Linux.
- **Desktop window**: 560x980, always-on-top, transparent, borderless fullscreen on Linux. Wayland: layer shell for click-through.
- **Desktop plugins**: DefaultPlugins, EguiPlugin, VrmPlugin, VrmaPlugin, ScenePlugin, EnePlugin, CharacterPlugin, TrayPlugin, SettingsUiPlugin, CharacterDragPlugin.
- **Release profile**: `codegen-units = 1`, `lto = "fat"`, `opt-level = "z"`, `strip = true`, `panic = "abort"`.
- **Dev profile**: `opt-level = 1` globally, `opt-level = 3` for dependencies.
- **Sandbox**: Path normalization → directory allowlist → blocked_commands → execute with limits. Blocked: `rm -rf /`, `dd if=`, `mkfs`, `sudo`, fork bombs. SQLite-backed undo with zlib compression.

## Configuration
- Settings loaded from JSON via `figment`. Loading order: defaults → `assets/settings.json` → env vars (`ENE_` prefix). Schema auto-generated as `settings.schema.json`.
- **Top-level `EneSettings`**: `version`, `character`, `user_name`, `runtime_rules`, `extra`
- **Sections**:
  - `provider` — LLM: `provider_name`, `model`, `base_url`, `api_key`
  - `embedding` — Vector: `provider_type` (api/local), `model`, `base_url`, `dimensions`, `gguf_quantization`
  - `memory` — Long-term: `enabled`, `db_path`, `recall_limit`, `similarity_threshold`, `time_decay_hours`, `summarization_model`
  - `session` — Split: `auto_session_split`, `session_timeout_minutes`, `topic_change_threshold`, `min_turns_before_split`
  - `sandbox` — Security: `enabled`, `allowed_directories`, `writable_directories`, `blocked_commands`, `max_read_bytes`, `shell_timeout_ms`
  - `tools` — Tool config: `tool_calling_enabled`, `max_tool_call_rounds`, `tools.<name>.enable/config`
  - `mcp_servers` — MCP: stdio/http transport configs
  - `desktop` — GUI: `graphics` (mask_render_downsample, target_fps, shadow_quality, antialiasing_mode)
- Character cards: `CharacterCardV3` format (spec, name, description, personality, scenario, system_prompt, first_mes, mes_example, extensions, assets), loaded from CLI args or auto-discovered.
- Resource dirs created on first run via `ensure_resource_dirs()`.

## Memory System
- **Storage**: SQLite + sqlite-vec + Diesel. Connection pooled via `r2d2`.
- **Tables**: `conversation_summaries` (embedding f32 blob), `conversation_keyfacts` (upsert), `conversation_logs`, `tool_embeddings`.
- **Search**: Cosine similarity via `vec_distance_cosine`. Results weighted by `similarity_weight` + `recency_weight` with time decay.
- **Embedding providers**: `ApiEmbeddingProvider` (OpenAI-compatible), `GgufEmbeddingProvider` (candle/GGUF, local, GPU-free).
- **Summarization**: Dedicated LLM model (`memory.summarization_model`) produces structured summary + topics + key_facts.

## Testing
- `cargo test --workspace` for all tests.
- REPL commands for interactive testing:
  - `/tooltest` — Tool test mode (launch with `-- --tooltest`)
  - `/tool call [tool_name] [args]` — Call a tool directly
  - `/tools` — List available tools
  - `/memory search [query]` — Search long-term memory
  - `/session info` — Current session details
  - `/session split` — Manual session split

## Docs
- Full documentation in `docs/` (also Japanese translations in `docs/ja/`). Key files:
  - `docs/architecture/overview.md` — Crate map, dependency graph, actor architecture
  - `docs/architecture/startup.md` — Boot sequences for desktop and CLI
  - `docs/configuration/settings.md` — Full settings.json schema reference
  - `docs/core/streaming.md` — Actor streaming, EneHandle/EneCommand/EneEvent lifecycle
  - `docs/core/prompt.md` — Prompt construction, CBS macros, expression protocol
  - `docs/core/session.md` — ConversationSession, CharacterCardV3
  - `docs/core/session-split.md` — Split triggers, max-pooling embedding, lifecycle
  - `docs/core/emotions.md` — Emotion token parsing, per-app display
  - `docs/memory/memory.md` — SQLite-vec memory, summarization, embedding providers
  - `docs/tools/overview.md` — Tool system architecture, IPC protocol, Tool RAG
  - `docs/tools/sdk.md` — Custom tool SDK guide (ToolProvider trait, IPC lifecycle)
  - `docs/tools/sandbox.md` — Security sandbox, blocked commands, undo system
  - `docs/applications/cli.md` — CLI REPL reference (15 commands)
  - `docs/applications/desktop.md` — Desktop app, Bevy plugins, VRM pipeline
