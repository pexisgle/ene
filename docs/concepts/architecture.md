# Architecture

This page explains how Ene is put together: what the processes are, what
each crate owns, and how one chat turn flows through the system.

## Big picture

Ene is a Cargo workspace with two applications, a set of library crates,
and a set of **out-of-process plugins**.

```text
┌────────────────────────────────────────────────────────────┐
│ Host process (ene-desktop or ene-cli)                      │
│                                                            │
│  ene-runtime  ── actor facade (EneHandle), events, tools   │
│    ├─ ene-mind      cognitive engine (prompt, memory,      │
│    │                emotion, recall, proactive)            │
│    ├─ ene-store     SQLite persistence (sole DB owner)     │
│    ├─ ene-ai        provider traits, routing, retry        │
│    ├─ ene-rag       scoring/decay/tool-selection policy    │
│    ├─ ene-plugin-host  plugin process supervision          │
│    └─ ene-connector external-service credentials/gates     │
└───────────────┬────────────────────────────────────────────┘
                │ IPC (length-prefixed frames, protocol v7)
┌───────────────▼────────────────────────────────────────────┐
│ Plugin child processes                                     │
│  plugins/tool/*      filesystem, web, browser, git, …      │
│  plugins/provider/*  anthropic, openai, local-llm,         │
│                      kokoro, whisper, voicevox, …          │
└────────────────────────────────────────────────────────────┘

ene-vrm (crates/ene-vrm) is a standalone wgpu renderer used by
ene-desktop only.
```

Two design rules explain almost everything else:

1. **The LLM is an utterance generator.** Personality, memory, emotions,
   and behaviour live in Ene's own state (`ene-mind` + `ene-store`), not in
   the model. Swapping the model does not change who the character is.
2. **Isolation over convenience.** Plugins run as separate processes, the
   database is owned by exactly one crate, and panic-isolation keeps one
   failure from taking down the app.

## The crates and their jobs

| Crate | Job | Must not depend on |
|---|---|---|
| `ene-core` | Persistence-agnostic domain vocabulary (memory types, affect state, commitments, schedules) and the `MemoryPort` trait | anything internal |
| `ene-config` | Settings loading/schema, paths, prompt packs | — |
| `ene-card` | Character card containers (V3), PNG/CHARX import, per-character settings | `ene-config` |
| `ene-mind` | Cognitive engine: prompt composition, recall planning, memory writing, emotion, proactive speech, sessions | `ene-runtime`, `ene-plugin-host`, `ene-store` (production) |
| `ene-store` | The **sole** owner of SQLite/SeaORM: schema, migrations, vector search, backups | `ene-ai`, `ene-mind`, `ene-runtime` |
| `ene-ai` | LLM/embedding/STT/TTS/VAD provider traits, task routing, retry, context-window math | — |
| `ene-rag` | RAG policy: hybrid scoring, decay, tool selection (`tool` feature), workspace chunking | `ene-store`, `ene-mind` |
| `ene-runtime` | Actor-based host facade (`EneHandle`), turn orchestration, event bus, tools, schedules, undo | — |
| `ene-connector` | External-service connector framework: credentials, permission gates, webhooks | `ene-config`, `ene-plugin-proto` |
| `ene-plugin-proto` | Wire ABI: IPC protocol v7, tool types, capabilities | business logic |
| `ene-plugin` | Plugin authoring facade (`run_plugin_server`, traits, prelude) | `ene-runtime`/`ene-mind`/`ene-store` |
| `ene-plugin-macros` | Proc macros: `ToolAction`, `ToolSpec`, provider derives | — |
| `ene-plugin-host` | Plugin process supervision, capability routing, IPC provider bridges, MCP client | — |
| `ene-plugin-db` | Typed CRUD client for plugin binaries (talks to the host's DB over IPC) | — |
| `ene-infer` | Single-threaded local-model framework (worker thread, bounded queue, cancellation) | — |
| `ene-voice` | Local STT/TTS/VAD engines (Whisper, Kokoro, Silero) | — |
| `ene-vrm` | VRM 1.0 loader + wgpu renderer | `ene-mind`/`ene-runtime`/`ene-store` |
| `ene-util` | Pure helpers (truncation, HTML→Markdown) | — |

The full per-crate reference, including feature flags and dependency edges,
is in [Crate reference](../reference/crates.md).

## Key architectural boundaries

- `ene-store` is the only crate that opens a database connection. Everyone
  else goes through `ene_core::MemoryPort` (mind, runtime) or the IPC-based
  `ene-plugin-db` client (plugin binaries).
- `ene-mind` programs against `MemoryPort`, never against
  `ene-store` directly — integration tests are the only place they meet.
- `ene-rag` sits between them: scoring and decay are pure policy that
  neither persistence nor cognition may reimplement.
- Wire-ABI crates (`ene-plugin-proto`) contain no business logic; business
  logic never moves into plugin binaries that only declare schema.

## A chat turn, end to end

```text
1. User message → EneHandle::run (returns TurnId; Busy if another turn runs)
2. ene-mind::before_turn
   - load affect state, decay it to now
   - plan recall (intent from the new message) and run hybrid memory search
   - load character identity + lorebook, scene summary, commitments
3. Compose the prompt packet
   - sectioned system prompt (identity kernel, memory, workspace chunks,
     commitments, style examples, output contract)
   - conversation history budgeted to the model's context window
4. Stream tokens from the LLM provider
   - mid-turn tool calls may run (files, web, …) with permission gates
   - performance cues are emitted for the avatar
5. ene-mind finalizes the turn
   - affect proposal (deterministic + optional LLM classifier)
   - session history committed to ene-store
6. EneEvent::Terminal emitted; deferred work continues in background
   - memory extraction & arbitration (candidates → store or approval queue)
   - forgetting/decay pass
```

The detailed pipeline is documented in
[Cognitive runtime](../reference/architecture/cognitive-runtime.md); the
host-facing contract is in [API v1](../reference/architecture/api-v1.md).

## Process model and fault tolerance

The host runs a Tokio actor for turn state. Every command and background
task is wrapped in `catch_unwind`, so a panic in one command is logged
(`DiagnosticEvent::ActorPanic`) and the actor keeps running. This is why
the release profile must **not** use `panic = "abort"` — unwinding is what
makes isolation possible.

Plugins are supervised child processes: the host spawns them, negotiates
the IPC protocol, registers their capabilities, probes their health, and
restarts them with a circuit breaker after repeated failures.

## Where state lives

- `memory.db` — SQLite database (conversation logs, typed memories +
  embeddings, affect states, commitments, schedules, audit log). Backups
  via `ene store backup` / `/store backup`.
- `undo.db*` — actor undo stack.
- `assets/characters/<name>/` — character cards and their assets.
- `assets/models/` — downloaded local models (gitignored).

See [Configuration](../configuration.md) for paths in debug vs release.
