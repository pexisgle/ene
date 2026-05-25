# CLI Reference (`ene-cli`)

Interactive REPL for chatting with AI characters, testing tools, and managing memory/sessions.

## Launch

```bash
cargo run -p ene-cli
# With tool test mode:
cargo run -p ene-cli -- --tooltest
# Set API key (stores in OS keyring):
cargo run -p ene-cli -- --set-api-key
```

## REPL Commands

Commands are prefixed with `/`:

### Session Commands

| Command | Action |
|---------|--------|
| `/quit` | Exit the REPL |
| `/clear` | Clear conversation history |
| `/history` | Show full conversation history |
| `/prompt` | Display the current system prompt |

### Character Commands

| Command | Action |
|---------|--------|
| `/card <path>` | Load a different character card |

### Config & Tools

| Command | Action |
|---------|--------|
| `/config` | Show current settings |
| `/tools` | List all enabled tools |
| `/undo` | Undo the last file operation |
| `/tooltest [prompt]` | Run a one-shot tool test |

### Memory Commands

| Command | Action |
|---------|--------|
| `/memory search <query>` | Search long-term memory |
| `/memory list` | List stored summaries and key facts |

### Session Split Commands

| Command | Action |
|---------|--------|
| `/session split` | Manually trigger a session split |
| `/session info` | Show session diagnostics |
| `/session summaries` | List past session summaries |

### Help

| Command | Action |
|---------|--------|
| `/help` | Show available commands |

## Stream Display

| Event | Styling |
|-------|---------|
| Normal text | Default stdout |
| `[Emotion: happy]` | Magenta |
| `[Tool Calling: name(args)]` | Cyan |
| `[Tool Result: ...]` | Green |
| `[Session split]` | Yellow |
| Errors | Red bold |

## Architecture

```
main.rs → clap arg parse
  → config::init() → load settings, AiRuntime::init()
  → repl::run() → dialoguer input loop
      → process_stream() → handle AiStreamEvent variants
      → commands::execute() → handle /-prefixed commands
```
