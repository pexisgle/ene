# System Overview

ene is a local AI character platform: chat with an LLM, optional tools, long-term memory, and (on desktop) a VRM avatar.

## Pieces that matter

| Piece | Role |
|-------|------|
| **CLI / Desktop** | Host apps. Load settings + character card, open the runtime, show UI. |
| **`ene-runtime`** | Host facade. One ready handle (`EneHandle::open`), one turn at a time. |
| **`ene-mind`** | Decides what goes into the prompt, recalls memory, updates affect, emits avatar cues. |
| **`ene-ai`** | Talks to cloud or local LLM / embedding backends. |
| **`ene-store`** | SQLite persistence for memory (and related indexes). |
| **Tools** | Separate processes over IPC (filesystem, web, browser, …). |
| **`ene-vrm`** | Renders the avatar; does not depend on mind/runtime. |

## One conversation turn

```text
You type a message
  → runtime starts a turn (or says Busy)
  → mind recalls context and builds the prompt
  → LLM streams tokens
  → tools may run mid-turn
  → mind writes memory / affect and emits Performance cues
  → turn ends (Terminal event)
```

Hosts subscribe to a small chat event bus; diagnostics are a separate channel.

## Where apps fit

- **CLI** — REPL, slash commands, good for debugging tools and memory.
- **Desktop** — same runtime, plus VRM playback driven by Performance events.

## Dig deeper

- Concepts: [Turns](concepts/turns.md), [Sessions](concepts/sessions.md), [Memory](concepts/memory.md), [Emotions](concepts/emotions.md)
- Design contracts: [Architecture overview](../reference/architecture/overview.md), [API v1](../reference/architecture/api-v1.md)
- Public APIs: [API index](../reference/api/index.md)
