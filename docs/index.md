# ene Documentation

ene is a local AI character platform implemented as a Rust workspace. It provides LLM-driven conversations with animated VRM characters, tool-augmented agent capabilities, long-term memory, and automatic session management.

## Getting Started

- [Architecture Overview](architecture/overview.md) — Crate map and dependency graph
- [Startup Flow](architecture/startup.md) — Desktop (winit+wgpu+egui) and CLI boot sequences
- [Cognitive Runtime Architecture](architecture/cognitive-runtime.md) — ADR for the Identity Kernel, typed memory, affect, and expression arbitration design
- [API Refactor Plan](architecture/api-refactor-plan.md) — Ongoing plan for restructuring the crate API surface
- [Configuration](configuration/settings.md) — Full settings.json schema reference
- [API Reference](api/index.md) — Public API documentation for every library crate

## Core Engine

| Document | Topic |
|----------|-------|
| [Streaming Engine](core/streaming.md) | Actor-based architecture, `EneHandle`, `EneEvent`, tool calling loop |
| [Streaming Events](core/streaming-events.md) | Which `EneEvent` variants fire on the legacy vs. cognitive streaming path |
| [Prompt Construction](core/prompt.md) | Message assembly order, system prompt, emotion protocol, function calling |
| [Session Management](core/session.md) | `ConversationSession`, `CharacterCardV3`, CBS macro expansion |
| [Session Splitting](core/session-split.md) | Timeout, topic change detection, manual split, async lifecycle |
| [Emotion Tokens](core/emotions.md) | `<\|emo:name\|>` parsing, VRM blendshape mapping |

## Memory

- [Long-Term Memory](memory/memory.md) — `MemoryStore`, embedding, vector search, summarization

## Tools

- [Tool System Overview](tools/overview.md) — IPC architecture, `ToolHostManager`, Tool RAG
- [Filesystem Tools](tools/fs.md) — `read`, `write`, `edit`, `glob`, `grep`, `patch`, `shell`, `undo`
- [Web Tools](tools/web.md) — `webfetch`, `websearch`
- [Utility Tools](tools/utility.md) — `question`, `todo`, `get_current_time`, `get_system_info`
- [GUI Automation](tools/app.md) — `app` mega-tool (15 actions)
- [Browser Automation](tools/browser.md) — `browser` mega-tool (8 actions via CDP)
- [Security Sandbox](tools/sandbox.md) — Path restrictions, blocked commands, undo system
- [Tool RAG](tools/tool-rag.md) — Embedding-based tool selection, HyDE, reranking
- [SDK Guide](tools/sdk.md) — Building third-party tools with `ene-tool-proto`
- [Derive Macro](tools/derive-macro.md) — `#[derive(ToolSpec)]` attribute reference

## Applications

- [CLI Reference](applications/cli.md) — REPL commands, flags, keyboard shortcuts
- [Desktop App](applications/desktop.md) — winit+wgpu+egui shell, VRM pipeline, overlay behavior

## Crate Index

| Crate | Type | Description |
|-------|------|-------------|
| `ene-config` | Library | Configuration, schemas, character cards, macros |
| `ene-core` | Library | Actor-based runtime, LLM streaming, tool orchestration, memory integration |
| `ene-cognition` | Library | Cognitive runtime — Identity Kernel, typed memory, affect, expression arbitration, commitments |
| `ene-embedding` | Library | Embedding providers (API + local GGUF) |
| `ene-memory` | Library | SQLite-vec memory store |
| `ene-session` | Library | Conversation history, session splitting |
| `ene-provider` | Library | LLM and embedding provider traits, OpenAI implementation |
| `ene-tool-proto` | Library | IPC protocol, `ToolProvider` trait, `ToolSpec`, `ToolError` |
| `ene-tool-derive` | Proc-macro | `#[derive(ToolSpec)]` for auto-generated tool specs |
| `ene-tool-host` | Library | Tool process manager, MCP support, Tool RAG |
| `ene-tool-db` | Library | Per-tool DB IPC client (used by tool binaries) |
| `ene-tool-common` | Library | Shared utilities (`ToolAction` trait, HTML extraction) |
| `ene-vrm` | Library | VRM 1.0 model loader and MToon renderer (used by `ene-desktop`) |
| `ene-tool-utility` | Binary | Utility tools (question, todo, time, system info) |
| `ene-tool-fs` | Binary | Filesystem tools (read, write, edit, shell, undo) |
| `ene-tool-web` | Binary | Web tools (fetch, search) |
| `ene-tool-app` | Binary | GUI automation (keyboard, mouse, screenshot) |
| `ene-tool-browser` | Binary | Browser automation (Chromium CDP) |
| `ene-cli` | Binary | Interactive CLI REPL |
| `ene-desktop` | Binary | winit + wgpu + egui desktop shell with VRM rendering |
