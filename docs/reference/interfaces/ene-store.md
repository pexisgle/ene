# `ene-store` interface

## Role

The **sole owner** of SQLite/SeaORM: schema, migrations, entities, vector
search (`sqlite-vec`), backups, audit log, and the DB IPC server that serves
plugin binaries.

## Public modules

| Module | Contents |
|---|---|
| `store` | `MemoryStore` (open, memory/affect/commitment/session/tool/workspace operations), conversation log types |
| `typed_memory` | `MemoryItem`, `Query`, `MemoryKind` etc. (re-exported from `ene-core`), store-side search DTOs |
| `entities` | SeaORM entity structs (one per table: `typed_memories`, `memory_embeddings`, `sessions`, `conversation_logs`, `commitments`, `schedules`, `audit_log`, …) |
| `migrator` | Versioned `SeaORM` migrations (`MigratorTrait`) |
| `port` | `impl MemoryPort for MemoryStore` and the other `ene-core` ports |
| `db_server` / `host_service` | The `db` passenger: IPC request handling for `ene-plugin-db` clients |
| `backup` | `OpenOptions`, `list_backups`, `restore_database` |
| `export` | `SessionExport`, `ExportedMessage`, `SESSION_EXPORT_FORMAT_VERSION`, `redact_secrets` |
| `audit` | `AuditEntry`, `NewAuditEntry`, `AuditDecision`, redaction helpers |
| `affect`, `commitment`, `schedule`, `session` | Store-side domain models for each area |
| `search`, `forgetting`, `config`, `error` | Hybrid scoring helpers, lifecycle transitions, `StoreConfig`, `EneMemoryError` |

## Key types

- `MemoryStore` — the concrete store; implements `ene_core::MemoryPort` and
  the other port traits.
- `EneMemoryError` — `#[non_exhaustive]` error enum; new internal variants
  project into `PublicApiError` automatically (see
  [API v1](../architecture/api-v1.md)).
- `StoreConfig` — `store.enabled` and related toggles.
- `SessionMeta`, `NewSessionMeta` — session listing metadata (mirrored by
  `PublicSessionMeta`).

## Dependencies

- Depends on: `ene-config`, `ene-core`, `ene-rag` (scoring core),
  `ene-plugin-db`/`ene-plugin-proto` (DB IPC wire types).
- Used by: `ene-runtime`, `ene-cli`, `ene-desktop`; plugin binaries through
  the `db` host-service passenger; `ene-mind` tests (dev).
- Explicitly **not** depended on: `ene-ai`, `ene-mind`, `ene-runtime`.

## Refactoring notes

- **Schema changes = migrations.** Entities are SeaORM; altering a table
  means a new migration in `migrator`, never editing old ones.
- The **DB IPC contract** (via `ene-plugin-db`) must stay additive:
  `DbRequest`/`DbResponse` variants and fields only grow (see
  [ene-plugin-db](ene-plugin-db.md)).
- The store takes embeddings as inputs; it never calls an embedder. Keep
  `ene-store` free of `ene-ai` imports — the dependency direction is the
  whole point.
- `memory.db` file format is versioned; backups/restore and integrity checks
  are part of the interface for operators (CLI `/store`, `ene store`).
- Redaction (`redact_secrets`, `redact_arguments`) is applied at the store
  boundary so secrets never reach logs or exports — keep it there.
