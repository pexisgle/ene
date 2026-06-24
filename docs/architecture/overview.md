# Architecture Overview

ene is a modular Rust workspace centered around the `ene-core` library, which ties together LLM integration, tool invocation, long-term memory, and session management through an **actor-based message-passing architecture**.

## Crate Dependency Graph

```
ene-desktop ──┬── ene-core ──── ene-tool-host ──── ene-tool-proto
ene-cli ──────┘       │           │     │     │          │
                      │           │     │     │          └── ene-tool-derive
                      │     ene-tool-utility, ene-tool-fs, ene-tool-web,
                      │     ene-tool-app, ene-tool-browser  (proc-macro)
                      │     (IPC subprocesses)
                      │
                ene-core internal deps:
                  ├── ene-config    (settings, paths, schema generation)
                  ├── ene-embedding (local GGUF + cloud embedding)
                  ├── ene-memory    (long-term memory store, sea-orm/sqlite-vec)
                  ├── ene-session   (conversation history, auto-split)
                  ├── ene-provider  (LLM + embedding traits, OpenAI impl)
                  ├── ene-tool-host (tool process management, MCP, Tool RAG)
                  ├── ene-tool-common  (shared tool utilities, ToolAction trait)
                  └── ene-tool-db   (per-tool DB IPC client, used by tool binaries)

ene-desktop only:
  ├── ene-vrm    (VRM 1.0 loader + MToon renderer)
  ├── winit      (windowing / event loop)
  ├── wgpu       (GPU rendering)
  ├── egui       (settings UI)
  └── bevy_ecs / bevy_app 0.19 (ECS scheduler only; no Bevy plugins)
```

## Layer Descriptions

### Configuration Layer
- **`ene-config`** — JSON-based settings with `figment`. Provides `define_config!` and `define_label_enum!` macros for declarative config structs. Manages platform-aware path resolution and automatic `settings.schema.json` generation.

### Core Runtime Layer
- **`ene-core`** — The unified runtime facade. Uses an **actor-based architecture** with channel-based message passing. `EneHandle` is the public API that spawns a background `EneActor` task. Consumers communicate via `EneCommand` (mpsc) and receive events via `EneEvent` (broadcast). The actor owns the session, config, and tool registry, and manages streaming, tool orchestration, permissions, and session splitting internally.

### AI Subsystems
- **`ene-embedding`** — Vector embedding generation. Two backends: `CloudEmbeddingProvider` (OpenAI-compatible API) and `GgufEmbeddingProvider` (candle/GGUF, local, GPU-free).
- **`ene-memory`** — SQLite + sqlite-vec ephemeral memory. Stores conversation summaries, key facts, and tool embeddings with cosine-similarity vector search.
- **`ene-session`** — Conversation history buffer, `CharacterCardV3` loading, emotion token parsing (`<|emo:name|>`), and automatic session splitting based on timeouts and topic drift.

### Tool Infrastructure Layer
- **`ene-tool-proto`** — Protocol contract. Defines `ToolProvider` trait, `ToolSpec`/`ToolError` types, `IpcRequest`/`IpcResponse` wire format (current `IPC_PROTOCOL_VERSION = 1`), `SandboxConfigData`, and the `run_tool_server()` helper.
- **`ene-tool-derive`** — Proc-macro crate. `#[derive(ToolSpec)]` generates `ToolSpec` implementations from declarative attributes on args structs.
- **`ene-tool-host`** — Tool lifecycle manager. Spawns tool binaries as child processes (Unix Domain Sockets / Windows Named Pipes), wraps them with crash resilience (exponential backoff, max 5 restarts), supports MCP servers, and provides Tool RAG filtering via the `ToolRag` struct (HyDE, LLM reranking, per-category limits). Also hosts `DbIpcServer` (Unix) that mediates per-tool SQLite access.
- **`ene-tool-db`** — Per-tool DB IPC client library. Tool binaries link this instead of `ene-memory`; they open a Unix socket to the host's `DbIpcServer` and execute SQL through it (prefix-based access control).
- **`ene-tool-common`** — Shared utilities consumed by tool crates (`ToolAction` trait, HTML-to-Markdown extraction).
- **`ene-vrm`** — VRM 1.0 model loader and MToon renderer used by `ene-desktop`. Standalone library with no dependency on `ene-core`.
- **`ene-provider`** — LLM and embedding provider traits (`LlmProvider`, `EmbeddingProvider`), OpenAI-compatible implementation, `HybridRerankProvider` for HyDE/rerank.

### Tool Providers (IPC Subprocesses)
- **`ene-tool-fs`** — Filesystem operations: `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch`, `shell`, `undo`. All operations respect the sandbox configuration.
- **`ene-tool-web`** — Web access: `webfetch` (URL → text/markdown/html) and `websearch` (multiple backends).
- **`ene-tool-utility`** — Utility tools: `question`, `todo`, `get_current_time`, `get_system_info`.
- **`ene-tool-app`** — OS-level GUI automation: window management, keyboard/mouse input, screenshots, clipboard.
- **`ene-tool-browser`** — Chromium automation via CDP: navigation, clicking, typing, content extraction, screenshots.

### Applications
- **`ene-cli`** — Interactive terminal REPL with `/` commands for session and memory management.
- **`ene-desktop`** — `winit` + `wgpu` + `egui` shell that renders a VRM character (via `ene-vrm`) with an always-on-top transparent overlay, system tray, and egui settings UI. The per-frame logic is owned by a `bevy_app::App` whose scheduler is driven by the winit event loop; no Bevy plugins or renderer are used.

## Data Flow

```
User Input
  ↓
Consumer sends EneCommand::Run { input }
  ↓
EneActor receives command
  ↓
Memory Search → build_messages()
  ↓
Spawn stream task → LLM API (stream)
  ↓
EneEvent pipeline (broadcast channel):
  → TextDelta → Display
  → SpecialToken → Emotion processing
  → ToolCallStart → Tool execution → ToolCallResult → LLM API (loop)
  → PermissionRequired → User approval → PermissionDecision
  → UserInputRequired → User response → UserInputResponse
  → Finished
  ↓
Stream task sends updated session back via oneshot
  ↓
Actor updates session, emits StatusChanged { Idle }
```

## Actor Architecture

The actor pattern ensures thread safety and clean separation of concerns:

| Component | Role |
|-----------|------|
| `EneHandle` | Thread-safe public API. Sends commands via mpsc, receives events via broadcast. |
| `EneActor` | Background task. Owns all mutable state (session, config, registry). |
| `EneCommand` | Consumer → Actor messages (Run, Cancel, Reconfigure, LoadCharacter, ListTools, CallTool, etc.) |
| `EneEvent` | Actor → Consumer events (TextDelta, ToolCall*, PermissionRequired, UserInputRequired, Finished, etc.) |
| `stream::run_stream` | Internal streaming engine spawned per Run command. Returns updated session via oneshot. |

Benefits:
- **No global state** — all state is owned by the actor
- **Thread-safe** — channel-based communication, no mutex contention on hot paths
- **Subscriber-friendly** — `try_recv()` for non-blocking polling (used by `ene-desktop`'s `AiBridge` drain task), `subscribe()` for multiple consumers
- **Lifecycle-managed** — actor exits when all handles are dropped
