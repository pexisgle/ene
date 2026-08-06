# CLI user guide

`ene-cli` is the terminal client: an interactive REPL for chatting with
characters, plus non-interactive subcommands for scripts and CI.

```sh
cargo run -p ene-cli -- <flags> [subcommand]
```

## Global flags

| Flag | Meaning |
|---|---|
| `--config <path>` | Load a different `settings.json` |
| `--character <name>` | Override the configured character |
| `--lang <en\|ja>` | Override the UI language |

With no subcommand, the REPL starts.

## The REPL

Type a message to talk to the character. Slash commands:

| Command | Usage | Purpose |
|---|---|---|
| `/help` | `/help` | List all commands |
| `/quit`, `/exit` | | Leave the REPL |
| `/clear` | `/clear` | Clear the screen |
| `/affect` | `/affect <show\|reset>` | Inspect/reset the character's PAD affect state |
| `/prompt` | `/prompt` | Show the composed prompt packet for the last turn (debug) |
| `/card` | `/card <name>` | Switch character card |
| `/characters` | `/characters` | List discovered characters |
| `/import` | `/import <path>` | Import a PNG/CHARX character card |
| `/config` | `/config [set <dotted.key> <value>]` | Show or mutate settings at runtime |
| `/history` | `/history` | Show conversation history |
| `/undo` | `/undo` | Undo the last state-changing operation |
| `/tool` | `/tool <list\|search\|help\|call>` | Inspect and call tools directly |
| `/memory` | `/memory <list\|inspect\|search\|why\|pin\|archive\|forget\|dispute\|restore\|status\|pending\|retry\|approval>` | Manage typed memories |
| `/commitments` | `/commitments <list\|done <id>>` | Manage the commitment ledger |
| `/session` | `/session <info\|split\|summaries\|list\|export\|import\|search\|archive\|unarchive>` | Manage sessions |
| `/permissions` | `/permissions <list\|revoke\|reset>` | Manage standing permission grants |
| `/connector` | `/connector <list\|status\|check\|connect\|disconnect\|grant\|revoke\|permissions>` | Manage external-service connectors |
| `/schedule` | `/schedule <list\|add\|history\|delete\|pause\|resume>` | Manage persistent schedules |
| `/doctor` | `/doctor` | Run environment health checks |
| `/greeting` | `/greeting [<index>\|none]` | Switch the greeting message |
| `/store` | `/store <backup\|list-backups\|restore\|integrity>` | Database backup/restore/integrity |
| `/workspace` | `/workspace <sync\|cancel\|status\|search <query>>` | Workspace RAG index management |

## Non-interactive subcommands

These run one operation and exit. Most accept `--json` for machine-readable
output.

### `ene run`

Run a single prompt and stream the response, then exit:

```sh
ene run "What's the weather?"
ene run --jsonl "Tell me a story"     # one JSON event object per line
ene run --json "Hello"                # single JSON summary
ene run --timeout 60 --yes "Delete /tmp/scratch"   # auto-approve tools, cap runtime
```

`--yes` auto-approves side-effecting tool operations (intended for
scripted, trusted environments). Without it, a permission-gated tool fails
the run instead of prompting. The prompt can also be read from stdin when
no prompt argument is given.

The `--jsonl` stream uses the API v1 event schema
([`PublicChatEvent`](../reference/architecture/api-v1.md)).

### `ene tool`

```sh
ene tool list
ene tool search "calendar"
ene tool help fs.write
ene tool call fs.read '{"path": "Cargo.toml"}'
```

### `ene session`

```sh
ene session list
ene session list --archived
ene session export <id>        # versioned, redacted JSON bundle
ene session import <path>
ene session search "query"
ene session archive <id>
```

### `ene characters`

```sh
ene characters list                # name, card path, assets
ene characters import <card.png|card.charx>
```

### `ene memory`

Query the typed-memory store:

```sh
ene memory list [--kind <KIND>]
ene memory inspect <id>
ene memory search "camping trip"
```

Full management (pin, archive, forget, dispute, restore, approval queue)
is available in the REPL via `/memory`.

### `ene doctor`

Environment health check: config validity, provider reachability, store
integrity, plugin state. Exit code reflects the result — useful for CI.

### `ene store`

```sh
ene store backup
ene store list-backups
ene store restore <path> --yes      # --yes confirms the destructive restore
ene store integrity
```

## Examples

```sh
# Chat with a different character non-interactively
ene --character "Mira" run "Good morning"

# Scripted: summarize the latest session into a file
ene session list --json | jq '.[0].id' | xargs ene session export

# Health gate for a cron job
ene doctor --json
```

## Localization

The CLI UI is localized (`en-US`, `ja`); `--lang` overrides the system
locale. Slash-command names and the JSONL event schema stay English.
