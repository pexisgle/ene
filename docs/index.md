# Ene Documentation

**Ene** is a local AI character platform implemented in Rust 2024: featuring LLM chat, rich tool plugins, long-term memory recall, local voice processing, and animated VRM avatars on desktop.

[日本語ドキュメント (Japanese)](ja/index.md)

---

## Documentation Structure

The documentation is organized into four clear sections:

| Section | Target Audience | Description |
|---|---|---|
| **[Getting Started](getting-started.md)** | Users & Developers | Installation, dependencies, building, and running CLI & Desktop apps. |
| **[Architecture](architecture.md)** | System Architects & Contributors | Workspace design, API v1 host contract, turn pipeline, IPC protocol v6. |
| **[Configuration](configuration.md)** | Operators & Developers | Full settings reference (`ENE_*` env vars, config files, character cards). |
| **[Concepts](concepts/turn-and-session.md)** | Developers | Deep-dives into turns, memory, voice/avatar, plugins, and MCP integration. |
| **[Crates Reference](crates/runtime.md)** | Developers & Contributors | Public API and internal architecture of the 17 workspace crates. |
| **[Applications](apps/cli.md)** | End Users | Guides for using `ene-cli` and `ene-desktop`. |

---

## Workspace Map

Ene is structured as a modular Cargo workspace composed of **17 crates**, **18 plugin binaries**, and **2 host applications**:

```text
Ene Workspace
├── Apps
│   ├── ene-cli            (CLI REPL application)
│   └── ene-desktop        (GUI desktop app with 3D VRM avatar & voice)
├── Core Engine
│   ├── ene-runtime        (Actor-based host facade & system turn engine)
│   ├── ene-mind           (Cognitive engine: session, prompt, affect, proactive, memory writer)
│   ├── ene-store          (SQLite + SeaORM + sqlite-vec memory & vector store)
│   ├── ene-config         (Settings, character cards, schema definition)
│   ├── ene-core           (Persistence-agnostic domain vocabulary & memory port)
│   ├── ene-ai             (Core AI provider traits, OpenAI, Anthropic adapter)
│   ├── ene-infer          (Local-model engine: worker threads, queues, conformance)
│   ├── ene-voice          (Local STT/TTS/VAD audio pipeline)
│   ├── ene-rag            (RAG policy: memory scoring/decay, tool selection)
│   ├── ene-connector      (External-service credential & identity authority)
│   ├── ene-util           (Pure utilities: truncation, HTML-to-Markdown)
│   └── ene-vrm            (3D VRM 1.0 loader & wgpu renderer)
├── Plugin Architecture
│   ├── ene-plugin-proto   (IPC wire protocol v6 definitions)
│   ├── ene-plugin         (Plugin authoring SDK & adapter facade)
│   ├── ene-plugin-host    (Plugin process manager & supervisor)
│   ├── ene-plugin-db      (Plugin host-service `db` client)
│   └── ene-plugin-macros  (Proc-macros for plugins)
└── Out-of-Process Plugins
    ├── plugins/provider/* (Provider plugins: anthropic, edge-tts, local-llm, openai, openai-tts, voicevox)
    └── plugins/tool/*     (Tool plugins: app, browser, calc, calendar, counter, fs, geo, git, homeassistant, random, utility, web)
```

---

## Navigation Quick Links

- [Setup & Running](getting-started.md)
- [System Architecture & Design](architecture.md)
- [Settings & Configuration](configuration.md)
- [Turns & Sessions](concepts/turn-and-session.md)
- [Memory & Recall](concepts/memory-system.md)
- [Voice & Avatar](concepts/voice-and-avatar.md)
- [Plugins & MCP System](concepts/plugins-and-mcp.md)
- [CLI User Guide](apps/cli.md)
- [Desktop User Guide](apps/desktop.md)
