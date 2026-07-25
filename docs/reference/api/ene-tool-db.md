# `ene-tool-db` — API Reference

> **Crate:** `ene-tool-db`
> **Role:** Typed CRUD database client for tool binaries — communicates with the `DbIpcServer` in `ene-runtime` via IPC.

---

## Overview

Tool binaries must **not** link `ene-store` directly. Instead, `ene-tool-db` provides a typed async client (`DbClient`) that speaks a length-prefixed JSON protocol to the `DbIpcServer` process running inside `ene-runtime`. The server enforces prefix-based access control so that each tool can only read and write tables whose name starts with its declared prefix (e.g. `fs_`, `utility_`).

All connection state (the underlying socket, plus the socket path and auth token needed to [`reconnect`](#reconnect)) lives behind `&mut self`: every CRUD method takes `&mut DbClient` because each call is a synchronous request/response round-trip over a single stream, and `reconnect` needs exclusive access to replace that stream after the server restarts.

See also: [Memory System Rules in AGENTS.md §7.3](../../../AGENTS.md) and [`ene-plugin-host`](./ene-plugin-host.md).

---

## `DbClient`

The primary handle to the database server.

```rust
pub struct DbClient { /* opaque */ }
```

### Constructors

| Constructor | Signature | Description |
|---|---|---|
| `connect` | `async fn connect(socket_path: &Path) -> Result<Self, DbError>` | Connects to the `DbIpcServer` at `socket_path` without authenticating. The server closes the connection on the first non-`Handshake` request it receives from an unauthenticated client, so this is only useful when the server is configured without an auth token. Prefer `connect_with_token` when a token is available. |
| `connect_with_token` | `async fn connect_with_token(socket_path: &Path, token: &str) -> Result<Self, DbError>` | Connects and immediately sends a `Handshake` request with `token`. Returns `Err(DbError::Auth { .. })` if the server rejects the token. The socket path and token are captured on the client so `reconnect` can later re-authenticate transparently. |

The socket path is provided to tool binaries via an environment variable set by `ene-plugin-host` when the process is spawned; the auth token is provided via `ENE_DB_AUTH_TOKEN`.

### Connection Management

| Method | Signature | Description |
|---|---|---|
| `socket_path` | `fn socket_path(&self) -> &Path` | Returns the socket path this client was connected to. |
| `reconnect` | `async fn reconnect(&mut self) -> Result<(), DbError>` | Re-establishes the IPC connection after the server has restarted, using the socket path and (if present) auth token captured at connect-time. Call this after receiving `DbError::ConnectionClosed` to recover without losing the client's identity — the previous design had no recovery path and required rebuilding the client from scratch. |

### Schema declaration

```rust
impl DbClient {
    /// Declare this tool's schema. Must be called once at startup, before any
    /// reads or writes.
    ///
    /// The server creates any tables and indexes that do not yet exist.
    /// Returns `(created_tables, created_indexes)`.
    pub async fn declare_schema(
        &mut self,
        schema: DbSchema,
    ) -> Result<(Vec<String>, Vec<String>), DbError>;
}
```

### CRUD methods

All CRUD methods are `async` and take `&mut self` (a single in-flight request per client at a time; the connection is not internally multiplexed).

| Method | Signature | Description |
|---|---|---|
| `insert` | `async fn insert(&mut self, table: &str, row: Row) -> Result<i64, DbError>` | Insert a row. Returns the new `rowid`. |
| `upsert` | `async fn upsert(&mut self, table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>` | Insert or update on conflict (`ON CONFLICT (conflict_columns) DO UPDATE`). Returns the `rowid`. |
| `select` | `async fn select(&mut self, table: &str, columns: &[&str], filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbError>` | Query rows. Pass `&[]` for `columns` to select all columns (`SELECT *`), and `vec![]` for `order_by` for no explicit ordering — there is no `Option`-wrapped "no filter"/"no order" sentinel; use the empty slice/vec instead. |
| `update` | `async fn update(&mut self, table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>` | Update matching rows. Returns the number of affected rows. |
| `delete` | `async fn delete(&mut self, table: &str, filter: DbFilter) -> Result<u64, DbError>` | Delete matching rows. Returns the number of deleted rows. |
| `count` | `async fn count(&mut self, table: &str, filter: DbFilter) -> Result<i64, DbError>` | Count matching rows. |
| `last_insert_rowid` | `async fn last_insert_rowid(&mut self) -> Result<i64, DbError>` | Returns the `rowid` of the most recently inserted row *on this connection*. |
| `ping` | `async fn ping(&mut self) -> Result<(), DbError>` | Health-check the connection. |
| `shutdown` | `async fn shutdown(&mut self) -> Result<(), DbError>` | Requests a graceful shutdown of the DB server and waits for its acknowledgement (`DbResponse::Ack`) — unlike a fire-and-forget send, a failure here (including `DbError::ConnectionClosed`) is visible to the caller. |

### Example

```rust,no_run
use ene_tool_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, DbOrderBy, DbValue, Row};
use std::path::Path;

async fn run() -> Result<(), ene_tool_db::DbError> {
    let mut client = DbClient::connect_with_token(
        Path::new("/run/ene/db.sock"),
        &std::env::var("ENE_DB_AUTH_TOKEN").unwrap_or_default(),
    ).await?;

    client.declare_schema(DbSchema {
        prefix: "fs".to_string(),
        tables: vec![
            DbTable {
                name: "fs_visited".to_string(),
                columns: vec![
                    DbColumn {
                        name: "id".to_string(),
                        ty: DbType::Integer,
                        primary_key: true,
                        auto_increment: true,
                        ..Default::default()
                    },
                    DbColumn {
                        name: "path".to_string(),
                        ty: DbType::Text,
                        ..Default::default()
                    },
                    DbColumn {
                        name: "visited_at".to_string(),
                        ty: DbType::Integer,
                        ..Default::default()
                    },
                ],
            },
        ],
        indexes: vec![],
    }).await?;

    // Insert
    let mut row = Row::new();
    row.insert("path".into(), DbValue::Text("/home/user/notes.md".into()));
    row.insert("visited_at".into(), DbValue::Int(1_700_000_000));
    let rowid = client.insert("fs_visited", row).await?;

    // Select (all columns, ordered by most-recently-visited first)
    let rows = client.select(
        "fs_visited",
        &[],  // SELECT * — empty slice, not None
        DbFilter::eq("path", "/home/user/notes.md"),
        vec![DbOrderBy::desc("visited_at")],
        Some(10),
    ).await?;

    // Delete
    let deleted = client.delete(
        "fs_visited",
        DbFilter::eq("id", rowid),
    ).await?;
    println!("Deleted {deleted} rows");

    client.shutdown().await?;
    Ok(())
}
```

---

## `DbError`

```rust
pub enum DbError {
    Transport(#[from] std::io::Error),
    UnexpectedResponse(String),
    ConnectionClosed,
    Auth { code: DbErrorCode, message: String },
    PermissionDenied { message: String },
    UnknownTable { message: String },
    UnknownColumn { message: String },
    TypeMismatch { message: String },
    InvalidFilter { message: String },
    Internal { message: String },
}
```

| Variant | Meaning |
|---|---|
| `Transport(std::io::Error)` | Low-level transport failure (write/read/serialize/deserialize). Implements `#[from] std::io::Error`, so `?` on raw I/O converts automatically. |
| `UnexpectedResponse(String)` | The server sent a syntactically valid but semantically wrong response variant for the request that was sent (e.g. an `Insert` request got back a `Select` response). Indicates a client/server protocol mismatch, not a data error. |
| `ConnectionClosed` | The IPC connection was closed unexpectedly (EOF while reading a response). Call [`reconnect`](#connection-management) to recover. |
| `Auth { code, message }` | The server rejected the auth token presented during `Handshake`, either on initial `connect_with_token` or during `reconnect`. Distinct from other query errors so callers can special-case "my token is stale/invalid". |
| `PermissionDenied { message }` | The tool does not have permission to access the requested resource (e.g., table name does not match the prefix). |
| `UnknownTable { message }` | The specified table does not exist or was not declared via `declare_schema`. |
| `UnknownColumn { message }` | The specified column does not exist in the table. |
| `TypeMismatch { message }` | A value's type does not match the column's declared type. |
| `InvalidFilter { message }` | The filter expression is invalid. |
| `Internal { message }` | An internal server error occurred. |

### `DbErrorCode`

```rust
pub enum DbErrorCode {
    PermissionDenied,
    UnknownTable,
    UnknownColumn,
    TypeMismatch,
    InvalidFilter,
    Internal,
}
```

| Variant | Meaning |
|---|---|
| `PermissionDenied` | The tool does not have permission to access the requested resource (e.g. attempted to read or write a table outside its declared prefix, or presented a bad auth token). |
| `UnknownTable` | The specified table does not exist or was not declared via `declare_schema`. |
| `UnknownColumn` | The specified column does not exist in the table. |
| `TypeMismatch` | A value's type does not match the column's declared type. |
| `InvalidFilter` | The filter expression is invalid (e.g. references a column that doesn't exist). |
| `Internal` | An internal server error occurred. |

`DbErrorCode` implements `Display`, rendering as `SCREAMING_SNAKE_CASE` (e.g. `PermissionDenied` → `"PERMISSION_DENIED"`).

---

## Schema Types

### `DbSchema`

```rust
pub struct DbSchema {
    /// Must match the tool's name prefix (e.g. "fs" for all "fs_*" tables).
    pub prefix: String,
    pub tables: Vec<DbTable>,
    pub indexes: Vec<DbIndex>,
}
```

### `DbTable`

```rust
pub struct DbTable {
    /// Must start with the schema's prefix followed by `_`.
    pub name: String,
    pub columns: Vec<DbColumn>,
}
```

### `DbColumn`

```rust
pub struct DbColumn {
    pub name: String,
    /// Column type. Defaults to `DbType::Text`.
    pub ty: DbType,
    /// Whether the column allows NULL. Defaults to `false`.
    pub nullable: bool,
    /// Whether this column is a primary key. Defaults to `false`.
    pub primary_key: bool,
    /// Whether this column auto-increments. Defaults to `false`.
    pub auto_increment: bool,
    /// Whether this column has a UNIQUE constraint. Defaults to `false`.
    pub unique: bool,
    /// Default value (typed). Defaults to `None`.
    pub default: Option<DbValue>,
}
```

`DbColumn` derives `Default`, so partial construction with `..Default::default()` (as in the example above) is idiomatic.

### `DbIndex`

```rust
pub struct DbIndex {
    pub name: String,
    pub table: String,
    pub columns: Vec<String>,
    pub unique: bool,
}
```

---

## Value Types

### `DbType`

```rust
pub enum DbType {
    Integer,
    Real,
    #[default]
    Text,
    Blob,
    Boolean,
}
```

| Variant | Meaning |
|---|---|
| `Integer` | 64-bit signed integer. |
| `Real` | 64-bit IEEE floating point. |
| `Text` | UTF-8 text string. Default for `DbColumn::ty`. |
| `Blob` | Binary blob. |
| `Boolean` | Boolean (stored as `INTEGER` `0`/`1`). |

### `DbValue`

```rust
pub enum DbValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Blob(Vec<u8>),
}
```

| Method | Signature | Description |
|---|---|---|
| `as_i64` | `fn as_i64(&self) -> Option<i64>` | Returns the value if it's an `Int`. |
| `as_str` | `fn as_str(&self) -> Option<&str>` | Returns the value if it's `Text`. |
| `as_bool` | `fn as_bool(&self) -> Option<bool>` | Returns the value if it's a `Bool`. |
| `as_f64` | `fn as_f64(&self) -> Option<f64>` | Returns the value if it's a `Float`. |
| `as_bytes` | `fn as_bytes(&self) -> Option<&[u8]>` | Returns the value if it's a `Blob`. |

`DbValue` implements `From<bool>`, `From<i32>`, `From<i64>`, `From<f64>`, `From<String>`, `From<&str>`, `From<Vec<u8>>`, and `From<&[u8]>`, so most call sites never construct variants directly — see the `DbFilter::eq("path", "/home/user/notes.md")` call in the example above, where the `&str` converts implicitly via the `impl Into<DbValue>` bound on `DbFilter::eq`.

### `Row`

```rust
pub type Row = BTreeMap<String, DbValue>;
```

A `Row` is an ordered map from column name to value. Using `BTreeMap` ensures deterministic column ordering in insert and upsert operations.

---

## Filter DSL

`DbFilter` is a recursive enum for building `WHERE` clauses without writing SQL strings.

```rust
pub enum DbFilter {
    Always,
    And(Vec<DbFilter>),
    Or(Vec<DbFilter>),
    Not(Box<DbFilter>),
    Eq  { column: String, value: DbValue },
    Ne  { column: String, value: DbValue },
    Lt  { column: String, value: DbValue },
    Le  { column: String, value: DbValue },
    Gt  { column: String, value: DbValue },
    Ge  { column: String, value: DbValue },
    In  { column: String, values: Vec<DbValue> },
    Like { column: String, pattern: String },
    IsNull    { column: String },
    IsNotNull { column: String },
}
```

### Constructors and combinators

Rather than building variants by hand, prefer the associated functions below, which take `impl Into<String>`/`impl Into<DbValue>` and so accept plain `&str`/numeric literals directly:

| Method | Signature | Description |
|---|---|---|
| `eq` | `fn eq(column: impl Into<String>, value: impl Into<DbValue>) -> Self` | `column = value`. |
| `ne` | `fn ne(column: impl Into<String>, value: impl Into<DbValue>) -> Self` | `column != value`. |
| `lt` / `le` / `gt` / `ge` | `fn lt(column: impl Into<String>, value: impl Into<DbValue>) -> Self` (and so on) | `<`, `<=`, `>`, `>=` comparisons. |
| `is_null` | `fn is_null(column: impl Into<String>) -> Self` | `column IS NULL`. |
| `is_not_null` | `fn is_not_null(column: impl Into<String>) -> Self` | `column IS NOT NULL`. |
| `and` | `fn and(self, other: DbFilter) -> Self` | Combines two filters with AND. Flattens nested `And`s on either side instead of nesting (`a.and(b).and(c)` produces one `And(vec![a, b, c])`, not `And(vec![And(vec![a, b]), c])`). |
| `or` | `fn or(self, other: DbFilter) -> Self` | Combines two filters with OR. Flattens nested `Or`s the same way `and` does. |
| `columns_referenced` | `fn columns_referenced(&self) -> BTreeSet<&str>` | Recursively collects every column name referenced anywhere in the filter tree, deduplicated and sorted. Useful for validating a filter against a known column set before sending it. |

`In { column, values }` and `Like { column, pattern }` have no dedicated
constructor function; construct them as struct-variant literals directly.

### Example filters

```rust,no_run
use ene_tool_db::{DbFilter, DbValue};

// WHERE path LIKE '/home/%' AND visited_at > 1700000000
let filter = DbFilter::Like {
    column: "path".into(),
    pattern: "/home/%".into(),
}.and(DbFilter::gt("visited_at", 1_700_000_000_i64));

// WHERE id IS NOT NULL
let filter = DbFilter::is_not_null("id");

// WHERE status IN ('active', 'pending')
let filter = DbFilter::In {
    column: "status".into(),
    values: vec![DbValue::Text("active".into()), DbValue::Text("pending".into())],
};

// No filter (SELECT all rows)
let filter = DbFilter::Always;
```

### `DbOrderBy`

```rust
pub struct DbOrderBy {
    pub column: String,
    pub direction: DbOrderDirection,  // Asc | Desc
}

impl DbOrderBy {
    pub fn asc(column: impl Into<String>) -> Self;
    pub fn desc(column: impl Into<String>) -> Self;
}
```

Pass a `Vec<DbOrderBy>` to `DbClient::select`; an empty `vec![]` means no explicit ordering (row order is otherwise unspecified — it is *not* guaranteed to be insertion order).

---

## Prefix Access Control

The `DbIpcServer` in `ene-runtime` enforces the following rule for every request:

> A tool may only access tables whose name begins with `<tool_prefix>_`.

If a tool attempts to read or write a table outside its prefix, the server returns `DbError::PermissionDenied { message }`.

This isolation is intentional: tools are separate, untrusted processes and must not be able to read or corrupt each other's state.

---

## See Also

- [`ene-plugin-host`](./ene-plugin-host.md) — Spawns tool binaries and provides the socket path / auth token used by `DbClient::connect_with_token`
- [`ene-plugin-proto`](./ene-plugin-proto.md) — Underlying `IpcStream` transport used by `DbClient`
- [`ene-tool-common`](./ene-tool-common.md) — Shared utilities available to tool binaries alongside `ene-tool-db`
