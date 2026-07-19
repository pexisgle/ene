# CLI Reference (`ene-cli`)

Interactive REPL for chatting with AI characters, testing tools, and managing memory/sessions.

## Startup

```bash
cargo run -p ene-cli
```

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
| `/tool help <name>` | Show detailed help for a tool |
| `/tool call <name> <json>` | Call a tool directly with JSON arguments |
| `/undo` | Placeholder (not yet supported with actor-based runtime) |

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
| `/memory status` | Show whether the typed memory store is enabled |

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

### Help

| Command | Action |
|---------|--------|
| `/help` | Show available commands |

## Stream Display

Progress and tool logs are rendered by a custom `tracing` layer as an ASCII tree when spans are nested (parallel pre-turn / post-turn work). Flat `tracing` events without an open span print as single lines. LLM text streams on stdout.

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

