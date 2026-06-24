# CLI Reference (`ene-cli`)

Interactive REPL for chatting with AI characters, testing tools, and managing memory/sessions.

## Startup

```bash
cargo run -p ene-cli
```

## Architecture

```
main.rs → clap args
  → config::init() → settings load, EneHandle::new()
  → AppContext { handle: EneHandle, commands: Vec<Arc<dyn CliCommand>> }
  → repl::run() → dialoguer input loop
      → stream::process_stream() → EneEvent dispatch
      → commands::execute() → / command dispatch via CliCommand trait
```

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
| `/memory search <query>` | Search long-term memory (embedding similarity) |
| `/memory list` | List stored summaries and key facts |

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

| Event | Style |
|-------|-------|
| Plain text | Default stdout |
| `[Emotion: happy]` | Magenta |
| `[Tool Calling: name(args)]` | Cyan |
| `[Tool Result: ...]` | Green |
| `[Session split]` | Yellow |
| Error | Red bold |
