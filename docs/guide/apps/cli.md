# CLI Reference (`ene-cli`)

Interactive REPL for chatting with AI characters, testing tools, and managing memory/sessions.

## Startup

```bash
cargo run -p ene-cli
```

## Command-line flags

| Flag | Action |
|------|--------|
| `-h`, `--help` | Print usage and exit |
| `-V`, `--version` | Print the version and exit |
| `--config <PATH>` | Load `settings.json` from an explicit path instead of the default location |
| `--character <NAME>` | Use a character card name or path instead of the configured default |
| `--lang <LANG>` | Override the UI language (`en` or `ja`); defaults to the system locale |

User-facing CLI output is localized through Fluent catalogs under `apps/ene-cli/i18n/{en-US,ja}/ene_cli.ftl`. The active language is negotiated from the system locale unless overridden with `--lang`.

## Non-interactive mode (#186)

With no subcommand, `ene` starts the interactive REPL. With a subcommand, it
runs a single operation against the runtime and exits — suitable for CI
pipelines and shell scripts. The global flags (`--config`, `--character`,
`--lang`) apply to both modes.

```bash
cargo run -p ene-cli -- run "hello"            # one prompt, stream text, exit
cargo run -p ene-cli -- run --jsonl "hello"    # one JSON event per line
cargo run -p ene-cli -- run --json "hello"     # single JSON summary object
cargo run -p ene-cli -- tool list --json
cargo run -p ene-cli -- session list --json
cargo run -p ene-cli -- memory search "tea" --json
cargo run -p ene-cli -- doctor --json
```

### Subcommands

| Subcommand | Action |
|------------|--------|
| `run [PROMPT…]` | Run a single prompt and stream the response, then exit. With no `PROMPT`, the prompt is read from stdin |
| `run --jsonl` | Emit one JSON object per line (streaming events) on stdout |
| `run --json` | Emit a single JSON summary object on stdout (conflicts with `--jsonl`) |
| `run --timeout <SECONDS>` | Abort if the turn does not complete within the given seconds |
| `run --yes` | Automatically approve side-effecting tool operations; without it a permission gate fails the run instead of prompting |
| `tool list\|search\|help\|call [--json]` | Manage and call tools (mirrors the REPL `/tool` commands) |
| `session list\|export\|import\|search\|archive\|unarchive [--json]` | Manage conversation sessions (mirrors `/session`) |
| `memory list\|inspect\|search [--json]` | Inspect cognitive memories (mirrors `/memory`) |
| `doctor [--json]` | Run environment health checks (mirrors `/doctor`) |
| `store backup\|list-backups\|restore\|integrity [--json]` | Backup / restore / integrity-check the memory database (#239) |

### Output contract

- Structured results (`--json` / `--jsonl`) print to **stdout**; logs and
  progress stay on **stderr**.
- `run --jsonl` emits a stable event stream mirroring the chat event bus:
  `turn_started`, `text_delta`, `performance`, `tool_call_start`,
  `tool_call_result`, `permission_denied`, and exactly one terminal `terminal`
  event (`reason` is `done`, `failed`, `cancelled`, or `timeout`).
- On failure, a JSON error envelope is printed to stdout:
  `{"error": {"code": "<class>", "message": "…"}}`. Error classes are
  `usage`, `runtime`, `timeout`, `tool_failed`, `busy`, and
  `confirmation_required`.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success |
| `1` | Generic runtime or execution failure |
| `2` | Invalid arguments or usage (matches clap) |
| `3` | The operation exceeded `--timeout` |
| `4` | A tool call failed |
| `5` | The runtime was busy or the actor was unavailable |
| `6` | A side-effecting operation required `--yes` confirmation |
| `130` | Interrupted by Ctrl-C |

## Architecture

```
main.rs → clap args
  → config::init() → ConfigStore::try_load, EneHandle::open(config, card)
  → AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
  → repl::run() → TerminalUi line editor loop
      → stream::process_stream() → EneEvent dispatch (TurnId-scoped)
      → commands::execute() → / command dispatch via CliCommand trait
```

Logs use a tree-aware `tracing` layer (`TreeLogLayer`) coordinated with `TerminalUi` so post-turn lines never overwrite the `>: ` prompt.
The CLI creates an `EneHandle` (actor) on startup. User input is sent via `handle.run()`, and events are received via `handle.subscribe()`.

### CliCommand Trait

Each `/` command implements the `CliCommand` trait:

```rust
#[async_trait]
pub trait CliCommand: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn usage(&self) -> &'static str;
    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String>;
}
```

Commands are registered in a `COMMANDS` slice and dispatched by name.

## REPL Commands

Commands are entered with `/` prefix:

### Session Commands

| Command | Action |
|---------|--------|
| `/quit` | Exit REPL |
| `/clear` | Note that conversation history will be refreshed on next run (manual clear is a no-op in this release) |
| `/history` | Show conversation history |
| `/prompt` | Show current system prompt (system, examples, memory, expression protocol) |

### Character Commands

| Command | Action |
|---------|--------|
| `/card <path>` | Load a different character card (async) |

### Config & Tools

| Command | Action |
|---------|--------|
| `/config` | Show current settings (provider, model, embedding, memory) |
| `/tool list` | List all registered tools |
| `/tool search <query>` | Search registered tools using RAG or name/description filter |
| `/tool help <name>` | Show detailed help for a tool |
| `/tool call <name> <json>` | Call a tool directly with JSON arguments |
| `/undo` | Undo the most recent reversible tool operation (filesystem write/edit/patch/delete). Irreversible operations (e.g. shell execution) are warned about and cannot be undone |
| `/permissions list` | List session-wide tool permission grants |
| `/permissions revoke <id>` | Revoke a single permission grant by id |
| `/permissions reset` | Revoke all session-wide permission grants |

### Memory Commands

| Command | Action |
|---------|--------|
| `/memory list [--kind <kind>]` | List typed memories (optional kind filter) |
| `/memory inspect <id>` | Show full typed-memory details |
| `/memory search <query>` | Search typed memories (hybrid score + breakdown) |
| `/memory why <id>` | Show recall/lifecycle debug context for a memory |
| `/memory pin <id>` | Pin memory to skip natural decay |
| `/memory archive <id>` | Mark memory as archived |
| `/memory forget <id>` | Mark memory as user-deleted |
| `/memory dispute <id>` | Mark memory as disputed |
| `/memory restore <id>` | Restore memory status to active |
| `/memory status` | Show whether the typed memory store is enabled (includes pending write counts) |
| `/memory pending` | List deferred memory-write retry queue rows |
| `/memory retry` | Force due and drain pending memory writes now |

### Affect Commands

| Command | Action |
|---------|--------|
| `/affect show` | Show current affect state |
| `/affect reset` | Reset affect state to neutral |

### Commitment Commands

| Command | Action |
|---------|--------|
| `/commitments list` | List active commitments |
| `/commitments done <id>` | Mark a commitment as done |

### Session Split Commands

| Command | Action |
|---------|--------|
| `/session split` | Manually split session (via actor ManualSplit command) |
| `/session info` | Session diagnostics |
| `/session summaries` | Past session summaries |
| `/session list` | List stored sessions (newest first) |
| `/session export <id>` | Export a session to a versioned, redacted JSON bundle |
| `/session import <path>` | Import a session from a JSON export file |
| `/session search <query>` | Full-text search over stored conversation messages |
| `/session archive <id>` | Archive a session |
| `/session unarchive <id>` | Unarchive a session |

### Diagnostics

| Command | Action |
|---------|--------|
| `/doctor` | Run environment health checks and print a colored summary |
| `/doctor --json` | Run the same checks and print machine-readable JSON |
| `/store backup` | Create a timestamped file backup of the memory database |
| `/store list-backups` | List `{db}.bak.*` backups (newest first) |
| `/store restore <path>` | Restore from a backup (shuts down and exits so the store can reopen) |
| `/store integrity` | Run `PRAGMA integrity_check` |

`/doctor` inspects the following categories and reports each check with a
status (`OK` / `WARN` / `ERROR` / `SKIP`) plus a remediation hint when a
problem is found:

| Category | Checks |
|----------|--------|
| Runtime | Actor responsiveness (snapshot round-trip, session, turn count) |
| Config | Character card loaded |
| AI Provider | Chat provider resolution and connectivity (lightweight models-list call with a ~5s timeout; no user data is sent). When `ai.fallback.enabled`, also probes every configured cloud provider's health (status, latency, last error) and reports the failover policy (#175) |
| Embedding | Embedding backend resolution (cloud or local) |
| Store | Memory store enablement and runtime availability |
| Tool Registry | Tool registration |
| Assets | Assets directory presence |

Secrets are never printed in full: API keys are shown as a short masked
prefix (e.g. `sk-…abcd` or `[redacted]`), and absolute private paths are
collapsed to `~/…` or a trailing component.

### Help

| Command | Action |
|---------|--------|
| `/help` | Show available commands |

## Stream Display

Progress and tool logs are rendered by a custom `tracing` layer as an ASCII tree when spans are nested (parallel pre-turn / post-turn work). Flat `tracing` events without an open span print as single lines. LLM text streams on stdout.

Each log line is colored by level (`INFO` green, `WARN` yellow, `ERROR` red) and shows a source label (`component` field when present, otherwise the short tracing target), for example `INFO MemoryWriter: …`. Span names in the tree are cyan.

| Channel | Content |
|---------|---------|
| stderr (tree / flat) | Pipeline phases, tools, post-turn memory / affect |
| stdout | `TextDelta` / `[Performance: …]` |

Post-turn work continues after `Terminal`. The REPL shows `>: ` immediately; later log lines are inserted **above** the prompt while preserving any in-progress input.

Example:

```text
>: hello
|- pre_turn.phase_a
| |- embedding
| | └ Generating user query embedding...
| └ ccv3_sync
|   └ Character card memories already up-to-date
assistant reply text
|- post_turn.memory
| └ Post-turn memory extraction and forgetting completed
>: 
```

