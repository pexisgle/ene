# Crate reference

This page is the authoritative map of every crate, app, and plugin binary
in the workspace, with the dependency rules that keep the architecture
intact. For how the pieces work together, see
[Architecture](../concepts/architecture.md).
For the **public interface of each crate** (modules, types, traits, and
refactoring seams), see [Crate interfaces](interfaces/overview.md).

## Applications

| Package | Path | Role |
|---|---|---|
| `ene-desktop` | `apps/ene-desktop` | GUI app: winit + wgpu + egui + bevy_ecs; avatar, chat, voice, tray, settings. Feature `voice` (default) gates cpal/rodio audio. |
| `ene-cli` | `apps/ene-cli` | Interactive REPL + non-interactive subcommands for scripting. |

## Library crates

| Crate | Role | Key dependencies (internal) |
|---|---|---|
| `ene-runtime` | Actor-based host facade: `EneHandle`, turn orchestration, 3-channel event bus, tools/schedules/undo/workspace handles, API v1 mirrors | mind, store, ai, plugin-host, rag, config, card, connector, core |
| `ene-mind` | Cognitive engine: prompt packets, recall, memory writing/arbiter, emotion, proactive, sessions, commitments, summarizer | core, config, card, ai, rag, util |
| `ene-store` | Sole SQLite/SeaORM owner: schema, migrations, sqlite-vec search, backups, audit, DB IPC server (`db` host-service passenger) | config, core, rag, plugin-db, plugin-proto |
| `ene-core` | Persistence-agnostic domain vocabulary + `MemoryPort`/`EmbeddingStorePort`/`WorkspaceDocumentPort` traits | (nothing internal) |
| `ene-card` | Character card containers (V3), PNG/CHARX import/export, per-character config, localized card diffs | config |
| `ene-config` | Settings load/save/schema, paths, prompts/patterns, `define_config!` macro | (nothing internal) |
| `ene-ai` | LLM/embedding/STT/TTS/VAD traits, task routing, retry, context-window math, model fetching | config, infer, plugin-proto |
| `ene-infer` | Single-threaded local-model framework (`LocalModel`, `EngineHandle`): worker thread, bounded queue, cooperative cancellation, panic recovery | (nothing internal) |
| `ene-rag` | RAG policy: hybrid scoring, decay, workspace chunking; feature `tool` adds tool-selection pipeline (needs ene-ai) | core, config |
| `ene-connector` | External-service connector framework: credentials, permission gates, policies, webhooks | (nothing internal) |
| `ene-plugin-proto` | Wire ABI: IPC protocol v7, tool types, capabilities, sandbox config | (nothing internal) |
| `ene-plugin` | Plugin authoring facade: `run_plugin_server`, `PluginDispatch`, traits, `prelude` | proto, infer, macros |
| `ene-plugin-macros` | Proc macros: `ToolAction`, `ToolSpec`, `tool_action`, provider derives | proto |
| `ene-plugin-host` | Plugin supervision: spawn/handshake/capabilities/health/circuit breaker, IPC provider bridges, MCP client, credential registry | proto, ai, config, connector |
| `ene-plugin-db` | Typed CRUD client for plugin binaries over the host `db` service | proto |
| `ene-voice` | Local voice engines: whisper STT, Kokoro TTS, Silero VAD (feature-gated: `local-stt`, `local-tts`, `silero-vad`) | ai, config, infer |
| `ene-vrm` | VRM 1.0 loader + wgpu renderer; standalone (used by ene-desktop) | (nothing internal) |
| `ene-util` | Pure helpers: truncation, HTML→Markdown (feature `html`) | (nothing internal) |

## Dependency rules (enforced by review, checked by CI)

```text
ene-core    ← ene-store, ene-mind, ene-rag     (vocabulary + ports)
ene-store   ↛ ene-ai, ene-mind, ene-runtime    (persistence stays pure)
ene-mind    ↛ ene-runtime, ene-plugin-host     (production code);
              calls persistence only via ene_core::MemoryPort
ene-rag     ↛ ene-store, ene-mind              (policy layer; no cycles possible)
ene-card    → ene-config                       (error/paths/language aliases only;
                                                never the reverse edge)
ene-plugin-proto ↛ business logic              (wire ABI only)
ene-vrm     ↛ ene-mind, ene-runtime, ene-store (renderer is standalone)
```

Violations of these edges are the most common way to break the repository.

## Plugin binaries

### Tool plugins (`plugins/tool/*`)

`app`, `browser`, `calc`, `calendar`, `counter`, `fs`, `geo`, `git`,
`homeassistant`, `random`, `utility`, `web` — see
[Built-in tools](../guides/tools/builtin-tools.md).

### Provider plugins (`plugins/provider/*`)

`openai`, `anthropic`, `local-llm` (llama.cpp; binary `ene-plugin-llama-cpp`),
`llama-server`, `onnx` (Silero VAD), `whisper` (whisper.cpp),
`kokoro` (ONNX TTS), `edge-tts`, `elevenlabs`, `openai-tts`, `voicevox` —
see [Plugins & MCP](../concepts/plugins-and-mcp.md).

## Feature flags that matter

| Feature | Owner | Effect |
|---|---|---|
| `tool` | `ene-rag` | Tool-selection RAG pipeline (pulls ene-ai/plugin-proto). Enabled by `ene-runtime`. |
| `local-stt` / `local-tts` / `silero-vad` | `ene-voice` | Native whisper/ONNX engines, consumed by provider plugins. |
| `voice` | `ene-desktop` | Microphone capture + playback (cpal/rodio). Off ⇒ inert stubs. |
| `test-util` | `ene-infer` | Conformance battery for `LocalModel` implementations. |
| `html` / `truncate` | `ene-util` | HTML→Markdown / truncation helpers. |

## Build and CI

- `default-members = ["apps/ene-cli"]` — a bare `cargo test`/`cargo clippy`
  covers only the CLI. Always pass `--workspace` or `-p <pkg>`.
- CI gates: `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, `cargo doc --workspace --no-deps`.
- Lints are the spec: `all`/`pedantic`/`cargo` denied, plus
  `unwrap_used`, `expect_used`, `panic`, `todo`, `dbg_macro`, … Exceptions
  must be `#[expect(lint, reason = "...")]` — `#[allow]` is rejected.
- Native deps come from the checked-in Nix flake; Windows is cross-compiled
  from Linux (mingw); macOS is unsupported.
