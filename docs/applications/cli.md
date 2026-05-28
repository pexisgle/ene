# CLI Reference (`ene-cli`)

Interactive REPL for chatting with AI characters, testing tools, and managing memory/sessions.

## Startup

```bash
cargo run -p ene-cli
# Tool test mode:
cargo run -p ene-cli -- --tooltest
```

## Architecture

```
main.rs → clap args
  → config::init() → settings load, EneHandle::new()
  → AppContext { handle: EneHandle }
  → repl::run() → dialoguer input loop
      → process_stream() → EneEvent dispatch
      → commands::execute() → / command dispatch
```

The CLI creates an `EneHandle` (actor) on startup. User input is sent via `handle.run()`, and events are received via `handle.subscribe()`.

## REPL Commands

Commands are entered with `/` prefix:

### Session Commands

| Command | Action |
|---------|--------|
| `/quit` | Exit REPL |
| `/clear` | Clear conversation history |
| `/history` | Show conversation history |
| `/prompt` | Show current system prompt |

### Character Commands

| Command | Action |
|---------|--------|
| `/card <path>` | Load a different character card (async) |

### Config & Tools

| Command | Action |
|---------|--------|
| `/config` | Show current settings |
| `/tools` | List enabled tools |
| `/undo` | Undo last file operation |
| `/tooltest [prompt]` | One-shot tool test |

### Memory Commands

| Command | Action |
|---------|--------|
| `/memory search <query>` | Search long-term memory |
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
