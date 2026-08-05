# Ene Documentation

Ene is a **local AI character platform** written in Rust. You chat with a
character that has its own identity, long-term memory, emotions, tools, and —
on the desktop — a live 3D avatar with voice.

Ene is built around one idea: the LLM is an *utterance generator*, not the
place where personality and memory live. A dedicated cognitive engine
(`ene-mind`) composes what the model sees, decides what it remembers, and
drives how it moves and speaks. The model itself is a swappable component
that can run in the cloud or fully local on your machine.

## What Ene can do

- **Chat with a character.** Characters are defined by "character cards"
  (the V3 community spec), which describe personality, scenario, example
  dialogue, lore, and optional Ene-specific extensions such as expressions
  and motions.
- **Remember.** Ene extracts typed memories from conversation (facts,
  preferences, events, promises, …), recalls the relevant ones on every
  turn, and forgets naturally over time. You can review and edit everything
  in a memory ledger.
- **Use tools.** File access, web search, calculators, calendar, browser
  control, Home Assistant, Git, and more run as isolated child processes.
  Side-effecting operations require your approval.
- **Connect to MCP servers.** Any Model Context Protocol server can be
  attached and exposed to the character as tools.
- **Talk.** Speech-to-text (Whisper), text-to-speech (Kokoro locally, or
  cloud voices), and voice-activity detection plug in as provider plugins.
- **Show a live avatar.** The desktop app renders a VRM 1.0 model with
  expressions, motions, lip-sync, look-at, spring bones, and beat sync.
- **Act on its own.** Proactive speech, persistent schedules, and
  commitment tracking let the companion start conversations or run tasks
  on a timer.

## Who this documentation is for

| If you are… | Start here |
|---|---|
| An end user who wants to run Ene | [Quickstart](quickstart.md) |
| Someone configuring characters and settings | [Configuration](configuration.md) → [Concepts](concepts/architecture.md) |
| A developer extending Ene with tools | [Write a tool](guides/tools/write-a-tool.md) |
| A developer integrating with the host API | [API v1 reference](reference/architecture/api-v1.md) |
| A contributor to this repository | [Architecture](concepts/architecture.md) → [Crate reference](reference/crates.md) |

## Documentation map

| Section | What it covers |
|---|---|
| [Quickstart](quickstart.md) | Build, configure, and run the CLI and desktop app |
| [Configuration](configuration.md) | `settings.json`, environment variables, file locations |
| [Concepts](concepts/architecture.md) | How Ene works: architecture, cards, memory, turns, plugins, voice |
| [Apps](apps/cli.md) | User guides for the CLI and desktop app |
| [Guides](guides/character-editor.md) | Task-oriented how-tos: editing characters, memory, schedules, tools |
| [Reference](reference/crates.md) | Precise technical references: contracts, protocols, APIs |

Japanese documentation is available at [日本語ドキュメント](ja/index.md).
Every page under `docs/` has a matching page under `docs/ja/`.

## Source of truth

This documentation is maintained against the **actual code** in this
repository. If a page and the code disagree, the code wins — please report
the discrepancy.

- Repository: <https://github.com/pexisgle/ene>
- Rust API docs: `cargo doc --workspace --no-deps`
