# `ene-runtime` interface

## Role

The actor-based host facade: `EneHandle` is how embedders open a character,
run turns, and consume events. It composes mind, store, AI, plugin host,
RAG, connectors, schedules, and undo.

## Public modules

| Module | Contents |
|---|---|
| `handle` | `EneHandle`, `EneEvent`, `EneEventReceiver`, `LifecycleEvent`, `LifecycleReceiver`, `AudioChunk`, `AudioStreamReceiver`, `EneStatus`, `EneStateSnapshot`, `ProviderCatalog`, `TerminalReason`, `DeferredToolTask`, `FeatureSettingsUpdate`, `MemoryLedgerChange`, `ShutdownTimeout` |
| `public_api` | `API_VERSION`, `PublicChatEvent`, `PublicLifecycleEvent`, `PublicSessionMeta`, `PublicExportedMessage`, `PublicPerfCue`, `PublicApiError`, redaction helpers |
| `types` | `TurnId`, `TurnOrigin`, `RunError`, `CancelError`, `RequestId` |
| `query` | `MemoryCandidateHandle`, `MemoryLedgerHandle`, `SessionQueryHandle` |
| `tools` | `ToolHandle` |
| `workspace` | `WorkspaceHandle`, `WorkspaceIndexer`, `WorkspaceStatusView` |
| `vision` | `VisionHandle` |
| `connectors` | `ConnectorHandle`, `ConnectorHandleError` |
| `diagnostics` | `EneDiagnostics`, `DiagnosticEvent`, `MemoryHandle` |
| `undo` | `UndoReport` |
| `bootstrap` | `EneHandle::open`, `open_from_disk`, `open_with_config`, `open_ready` |
| `error` | `EneRuntimeError` |
| `task_config` | `ToolRuntimeConfig` (bounded-task admission caps) |
| hidden | `streaming` (permission/stream internals), `message_builder`, `scheduler`, `proactive*` — `#[doc(hidden)]`, not part of the contract |

## Key `EneHandle` surface

- Open/shutdown: `open(config, card)`, `shutdown(timeout)`.
- Turns: `run(input) -> TurnId`, `cancel(&TurnId)`, `active_turn()`.
- Events: `subscribe()` (chat), `subscribe_lifecycle()`,
  `take_audio_stream()`.
- Character/session: `set_character`, `set_greeting`, `card_name`,
  `session_id`, `turn_count`, `history`, `compress_context`.
- Permissions/tools: `decide_permission`, `submit_user_input`,
  `list_permissions`, `revoke_permission`, `reset_all_permissions`,
  `undo`, `tools()`, `provider_catalog()`.
- Schedules: `add_schedule`, `list_schedules`, `list_schedule_runs`,
  `delete_schedule`, `set_schedule_enabled`.
- Read-only handles: `sessions()`, `candidates()`, `memory_ledger()`,
  `vision()`, `workspace()`, `connectors()`, `diagnostics()`.

## Dependencies

- Depends on: `ene-mind`, `ene-store`, `ene-ai`, `ene-rag` (with `tool`),
  `ene-plugin-host`, `ene-config`, `ene-card`, `ene-connector`, `ene-core`.
- Used by: `ene-cli`, `ene-desktop`, external embedders.

## Refactoring notes

- **API v1 is exactly** the `Public*` types plus five session methods
  (`list_sessions`, `export_session`, `import_session`, `search_sessions`,
  `archive_session`). Everything else on `EneHandle` is host-internal
  wiring and may change freely (see [API v1](../architecture/api-v1.md)).
- The actor model is load-bearing: commands and background tasks run
  through `catch_unwind`, and the release profile must keep
  `panic = "unwind"`. Do not refactor the isolation away.
- The three-channel event bus (chat / lifecycle / audio) exists so one
  traffic class cannot starve another; keep the split when restructuring
  events.
- Re-exported types from other crates (`EneConfig`, `LlmMessage`,
  `CharacterCardV3`, …) are conveniences, not the contract — prefer the
  owning crate when adding new API.
