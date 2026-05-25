# Architecture Overview

ene is a modular Rust workspace centered around the `ene-core` library, which ties together LLM integration, tool invocation, long-term memory, and session management.

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
        ├── ene-tool-host (tool process management, MCP)
        └── ene-tool-proto (protocol types, ToolProvider trait)
```

## Layer Descriptions

### Configuration Layer
- **`ene-config`** — JSON-based settings with `figment`. Provides `define_config!` and `define_label_enum!` macros for declarative config structs. Manages platform-aware path resolution and automatic `settings.schema.json` generation.

### Core Runtime Layer
- **`ene-core`** — The unified runtime facade. Wraps all subsystems behind a single `AiRuntime::init()` call. Provides `run_ai_with_tools()` for streaming LLM completions with tool orchestration.

### AI Subsystems
- **`ene-embedding`** — Vector embedding generation. Two backends: `ApiEmbeddingProvider` (OpenAI-compatible) and `GgufEmbeddingProvider` (candle/GGUF, local, GPU-free).
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

## Data Flow (Simplified)

```
User Input → AiRuntime → Memory Search → build_messages()
    → LLM API (stream) → AiStreamEvent pipeline
        → TextDelta → Display
        → ToolCall → IPC to tool binary → ToolCallResult → LLM API (loop)
        → Finished

Session Boundary Check → summarize_conversation() → MemoryStore.insert_summary()
```
