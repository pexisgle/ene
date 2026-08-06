# `ene-plugin-db` interface

## Role

Typed CRUD database API for **plugin binaries**, communicating with the
host's `memory.db` over the host-service `db` passenger. Feature-agnostic:
it knows tables, rows, and values, not business meaning.

## Public modules

| Module | Contents |
|---|---|
| `client` | `DbClient`, `DbError` |
| `messages` | `DbRequest`, `DbResponse`, `DbWriteOp`, `DbBatchOpResult`, `DbErrorCode` |
| `types` | `DbSchema`, `DbTable`, `DbColumn`, `DbType`, `DbFilter`, `DbValue`, `DbIndex`, `DbOrderBy`, `DbOrderDirection`, `Row` |

## Key API

- `DbClient::connect_with_token(socket, token)` — connect to the host
  service; `connect` for tests.
- `declare_schema(schema)` — register the plugin's table schema (prefix +
  tables + indexes).
- `insert` / `update` / `delete` / `search` — typed CRUD on the plugin's
  own tables.
- `batch(ops)` — run a list of `DbWriteOp` in one SQLite transaction
  (all-or-nothing).

## Dependencies

- Depends on: `ene-plugin-proto` (wire types).
- Used by: `ene-store` (server side), stateful plugin binaries
  (`plugins/tool/counter`, `calendar`, `utility`, …).

## Refactoring notes

- **Prefix isolation is the security model**: the server only lets a
  plugin touch tables under its declared prefix. Keep the check server-side
  and token-authenticated.
- Wire compatibility rules (from the crate docs): the DB channel carries no
  version field; `Batch` was an *additive* extension. Extend request/
  response enums additively only (`#[serde(default)]` for new optional
  fields).
- `DbValue` is the plugin-facing value language; mapping between it and
  `ene-core` domain types happens in `ene-store`, not here.
