# Reference

Design contracts, crate APIs, and implementation detail for AI agents and deep contribution work.

[← Docs home](../index.md) · [Developer Guide](../guide/index.md) · [日本語](../ja/reference/index.md)

## Read first (agents)

1. [API v1](architecture/api-v1.md) — locked host contract (`EneHandle::open`, `TurnId`, Busy, events)
2. [Architecture overview](architecture/overview.md) — crate map and turn flow
3. [API index](api/index.md) — public surface of every library crate
4. Then topic docs below as needed

## Architecture

| Document | Topic |
|----------|-------|
| [Overview](architecture/overview.md) | Crate map, turn flow |
| [API v1](architecture/api-v1.md) | Host contract ADR |
| [Startup](architecture/startup.md) | Desktop and CLI boot |
| [Cognitive runtime](architecture/cognitive-runtime.md) | Mind / memory / affect ADR |
| [Avatar Performance](architecture/avatar-performance.md) | `PerformanceCue` ADR |

## API

- [Crate API index](api/index.md) — dependency graph and per-crate pages

## Runtime

| Document | Topic |
|----------|-------|
| [Streaming](runtime/streaming.md) | Actor, `EneHandle`, tool loop |
| [Streaming events](runtime/streaming-events.md) | `EneEvent` variants |
| [Prompt](runtime/prompt.md) | `PromptPacket` assembly |
| [Session](runtime/session.md) | Session and character card |
| [Session split](runtime/session-split.md) | Split / compression lifecycle |
| [Emotions](runtime/emotions.md) | Emotion tokens and Performance |

## Memory, configuration, tools

- [Long-term memory](memory/memory.md)
- [Settings schema](configuration/settings.md)
- [Tool IPC / host](tools/overview.md) · [Tool RAG](tools/tool-rag.md) · [SDK](tools/sdk.md) · [Derive](tools/derive-macro.md)

Human-oriented tool catalog: [Guide → Tools](../guide/tools/overview.md).
