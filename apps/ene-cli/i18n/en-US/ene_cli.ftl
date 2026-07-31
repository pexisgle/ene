welcome = ene Interactive CLI
help-hint = Type '/help' for a list of commands.

## REPL / run errors
busy-warning = [Busy] A turn is already in progress. Wait for it to finish.
run-failed = [Run] Failed: { $error }
stream-lag-resync = [Stream] Dropped { $skipped } events (fell behind); cancelling the active turn to resynchronize.
unknown-command = Unknown command: { $command }

## Permission prompt
permission-prompt = Select a permission for this action
permission-allow-once = Allow once
permission-allow-session = Always allow this session
permission-deny = Deny

## User input prompt
user-input-select = Select an answer (arrow keys to move, Enter to confirm)
user-input-freetext = Free text (empty to skip, 'cancel' to cancel all)
user-input-skip = (skip)
user-input-cancel = (cancel all)

## /help command
help-commands-title = Commands:
help-quit = Exit the CLI

init-failed = Failed to initialize: { $error }
turn-failed = Error: { $detail }
runtime-error-no-character-card = Character card not found or could not be loaded.
runtime-error-channel-closed = Connection to the AI runtime was lost.
runtime-error-mind-prerequisite = A required component is missing: { $name }
runtime-error-bootstrap = Startup failed: { $message }
runtime-error-config = Configuration error: { $detail }
runtime-error-memory = Memory store error: { $detail }
runtime-error-mind = Mind engine error: { $detail }
runtime-error-tool = Tool error: { $detail }
runtime-error-actor-busy = Ene is busy handling other requests right now. Try again in a moment.
runtime-error-ai-auth = AI provider authentication failed. Check your API key.
runtime-error-ai-rate-limit = AI provider rate limit exceeded. Try again later.
runtime-error-ai-network = Could not reach the AI provider. Check your network connection.
runtime-error-ai-local-llm = Local model error: { $detail }
runtime-error-ai-busy = The AI provider is busy right now. Try again in a moment.
runtime-error-ai-provider = AI provider error: { $detail }
runtime-error-ai-embedding = Embedding provider error: { $detail }
runtime-error-turn-failed = { $detail }
runtime-error-turn-failed-unknown = The request failed for an unknown reason.
