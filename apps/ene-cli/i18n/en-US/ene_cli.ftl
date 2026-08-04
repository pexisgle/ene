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

## /greeting command
greeting-no-card = No character card loaded.
greeting-no-greetings = This character has no greetings.
greeting-history-not-empty = Greetings can only be chosen before the first message. Restart the REPL to open a new session.
greeting-none-selected = No greeting selected.
greeting-selected = Greeting selected:
greeting-failed = Failed to set greeting: { $error }
greeting-choose = Choose a greeting (Enter to confirm)
greeting-none = (none)

## /memory approval command
memory-approval-usage = /memory approval <list|inspect <id>|approve <id>|edit <id> --title <title> --content <content> --kind <kind> --confidence <0..1>|reject <id>|history> (use --content="multi word text" for spaced values)
memory-approval-list-title = Pending candidates awaiting approval ({ $count })
memory-approval-history-title = Resolved candidate history ({ $count })
memory-approval-empty = No pending candidates awaiting approval.
memory-approval-history-empty = No approved or rejected candidates yet.
memory-approval-not-found = Candidate { $id } not found or already resolved.
memory-approval-label-id = id
memory-approval-label-title = Title
memory-approval-label-kind = Kind
memory-approval-label-confidence = Confidence
memory-approval-label-reason = Reason
memory-approval-label-source-quote = Source
memory-approval-label-source-turn = Source turn
memory-approval-label-conflict = Conflicts with
memory-approval-label-status = Status
memory-approval-label-created = Created
memory-approval-label-resolved = Resolved
memory-approval-status-pending = pending
memory-approval-status-approved = approved
memory-approval-status-rejected = rejected
memory-approval-approve-ok = Approved candidate { $id }.
memory-approval-reject-ok = Rejected candidate { $id }.
memory-approval-edit-ok = Edited candidate { $id }.
memory-approval-edit-missing-flag = Edit requires --title, --content, --kind, and --confidence.
memory-approval-edit-invalid-confidence = Confidence must be a number between 0 and 1.
memory-approval-edit-invalid-kind = Unknown memory kind '{ $kind }'. Valid: episodic, semantic, user_profile, relationship, affective, commitment, preference, procedure, reflection.
memory-approval-error = Approval error: { $error }

## /card command
card-loaded = Character card loaded: { $name }

## /workspace command
workspace-sync-started = Workspace index sync started. Use '/workspace status' to watch progress and '/workspace cancel' to stop it.
workspace-sync-busy = A workspace sync is already running.
workspace-cancel-sent = Cancellation requested for the running workspace sync.
workspace-status-title = Workspace index status:
workspace-status-disabled = Workspace RAG is disabled (rag.workspace.enabled).
workspace-status-no-folders = No folders are configured; nothing will be scanned or searched.
workspace-status-folders = Permitted folders:
workspace-status-indexed = Indexed files: { $files }, chunks: { $chunks }
workspace-status-progress = Sync in progress ({ $phase }): scanned { $scanned }, indexed { $indexed }, skipped { $skipped }, chunks { $chunks }
workspace-status-report = Last sync: indexed { $indexed }, unchanged { $unchanged }, renamed { $renamed }, deleted { $deleted }, skipped { $skipped }, { $chunks } chunks in { $elapsed }s
workspace-status-last-error = Last sync error: { $error }
workspace-search-empty = No matching workspace documents found.
workspace-search-title = Matching workspace documents:

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
runtime-error-workspace = Workspace index error: { $detail }
runtime-error-actor-busy = Ene is busy handling other requests right now. Try again in a moment.
runtime-error-store-required = The scheduler requires the memory store. Enable `store.enabled` in your configuration.
runtime-error-ai-auth = AI provider authentication failed. Check your API key.
runtime-error-ai-rate-limit = AI provider rate limit exceeded. Try again later.
runtime-error-ai-network = Could not reach the AI provider. Check your network connection.
runtime-error-ai-local-llm = Local model error: { $detail }
runtime-error-ai-busy = The AI provider is busy right now. Try again in a moment.
runtime-error-ai-provider = AI provider error: { $detail }
runtime-error-ai-embedding = Embedding provider error: { $detail }
runtime-error-turn-failed = { $detail }
runtime-error-turn-failed-unknown = The request failed for an unknown reason.
