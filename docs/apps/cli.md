# `ene-cli` User Guide

`ene-cli` is the command-line REPL interface for chatting with Ene, inspecting memory, managing sessions, and testing tool plugins.

---

## Launching the CLI

```bash
# Run with default settings
cargo run -p ene-cli

# Run with custom character card
cargo run -p ene-cli -- --character assets/cards/ene.json

# Run with verbose tracing logs enabled
RUST_LOG=info cargo run -p ene-cli
```

---

## REPL Slash Commands

Inside the `ene-cli` interactive prompt, type `/` to access commands:

| Command | Description |
|---|---|
| `/help` | Display list of available REPL slash commands |
| `/prompt` | Preview the exact message list sent to the AI (rendered directly from `build_messages`) |
| `/memory list` | Display active session recalled memory facts |
| `/memory clear` | Purge or reset active session memories |
| `/tool list` | List registered IPC tool plugins & active MCP servers |
| `/tool call <name> <json>` | Execute a tool action directly from REPL |
| `/characters` | List characters discovered under `assets/characters/` |
| `/session list` | List active & past sessions in SQLite |
| `/session split` | Force an immediate session boundary split |
| `/quit` or `/exit` | Safely shutdown `ene-runtime` and exit |

---

## Non-interactive subcommands

### `ene characters list`

Lists the characters discovered under `assets/characters/` (the same rule the
desktop uses: a folder counts as a character when it contains
`character.json`).

```bash
# Human-readable
ene characters list

# Machine-readable JSON (name, folder, card/vrm/motion paths, default motion)
ene characters list --json
```

---

## Proactive (spontaneous) speech

When `mind.proactive.enabled = true`, Ene can speak spontaneously even while you are not typing. The REPL keeps a continuous subscription to the chat event bus, so proactive utterances are rendered above the prompt while the REPL is idle — the same behavior as the desktop app. If a proactive turn starts while you are in the middle of typing, the in-progress line is cancelled (its text is discarded) and the prompt resumes once the utterance finishes.
