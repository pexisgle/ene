# Architecture Overview

ene is a modular Rust workspace centered around the `ene-core` library, which ties together LLM integration, tool invocation, long-term memory, and session management through an **actor-based message-passing architecture**.

## Crate Dependency Graph

```
ene-desktop ──┐
ene-cli ──┼── ene-core ──── ene-tool-host ──── ene-tool-proto
            │                    │
            │               ene-tools/* (IPC subprocesses)
            │
      ene-core internal deps:
        ├── ene-config    (settings, paths, schema generation)
        ├── ene-embedding (vector embeddings)
        ├── ene-memory    (long-term memory store)
        ├── ene-session   (conversation history, auto-split)
        └── ene-tool-host (tool process management, MCP)
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
- **`ene-tool-proto`** — Protocol contract. Defines `ToolProvider` trait, `IpcRequest`/`IpcResponse` wire format, `SandboxConfigData`, and the `run_tool_server()` helper.
- **`ene-tool-host`** — Tool lifecycle manager. Spawns tool binaries as child processes (Unix Domain Sockets / Windows Named Pipes), wraps them with crash resilience (exponential backoff, max 5 restarts), supports MCP servers, and provides Tool RAG filtering via embedding similarity.
- **`ene-tools-common`** — Shared utilities consumed by tool crates (HTML-to-Markdown, smart truncation).

### Tool Providers (IPC Subprocesses)
- **`ene-tools-fs`** — Filesystem operations: `read`, `write`, `edit`, `delete`, `glob`, `grep`, `patch`, `shell`, `undo`. All operations respect the sandbox configuration.
- **`ene-tools-web`** — Web access: `webfetch` (URL → text/markdown/html) and `websearch` (multiple backends).
- **`ene-tools-utility`** — Utility tools: `question`, `todo`, `get_current_time`, `get_system_info`.
- **`ene-tools-app`** — OS-level GUI automation: window management, keyboard/mouse input, screenshots, clipboard.
- **`ene-tools-browser`** — Chromium automation via CDP: navigation, clicking, typing, content extraction, screenshots.

### Applications
- **`ene-cli`** — Interactive terminal REPL with `/` commands for session and memory management.
- **`ene-desktop`** — Bevy-based GUI with VRM character rendering, always-on-top transparent overlay, system tray, and egui settings UI.

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
| `EneCommand` | Consumer → Actor messages (Run, Cancel, Reconfigure, LoadCharacter, etc.) |
| `EneEvent` | Actor → Consumer events (TextDelta, ToolCall*, PermissionRequired, Finished, etc.) |
| `stream::run_stream` | Internal streaming engine spawned per Run command. Returns updated session via oneshot. |

Benefits:
- **No global state** — all state is owned by the actor
- **Thread-safe** — channel-based communication, no mutex contention on hot paths
- **Bevy-friendly** — `try_recv()` for non-blocking ECS polling, `subscribe()` for multiple consumers
- **Lifecycle-managed** — actor exits when all handles are dropped
