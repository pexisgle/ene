# `ene-tool-db`

> Typed CRUD database client for tool binaries — communicates with the `DbIpcServer` in `ene-core` via IPC.

Tool binaries must **not** link `ene-memory` directly. Instead, `ene-tool-db` provides a typed client that speaks to the `DbIpcServer` process running inside `ene-core`. The server enforces prefix-based access control so that each tool can only read and write tables whose name starts with its declared prefix (e.g. `fs_`, `utility_`).

See also: [Memory System Rules in AGENTS.md §7.3](../../AGENTS.md) and [`ene-tool-host`](ene-tool-host.md).

---

## `DbClient`

The primary handle to the database server.

```rust
pub struct DbClient { /* private */ }
```

### Constructor

```rust
impl DbClient {
    /// Connect to the DbIpcServer at the given socket path.
    ///
    /// The socket path is provided to tool binaries via an environment variable
    /// set by `ene-tool-host` when the process is spawned.
    pub async fn connect(socket_path: &Path) -> Result<Self, DbError>;

    /// Connect with an explicit auth token (overrides the
    /// `ENE_DB_AUTH_TOKEN` env var, which is the default source).
    pub async fn connect_with_token(
        socket_path: &Path,
        token: &str,
    ) -> Result<Self, DbError>;
}
```

### Schema declaration

```rust
impl DbClient {
    /// Declare this tool's schema. Must be called once at startup, before any
    /// reads or writes.
    ///
    /// The server creates any tables and indexes that do not yet exist.
    /// Returns `(created_tables, created_indexes)`.
    pub async fn declare_schema(
        &self,
        schema: DbSchema,
    ) -> Result<(Vec<String>, Vec<String>), DbError>;
}
```

### CRUD methods

| Method | Signature | Description |
|---|---|---|
| `insert` | `(table: &str, row: Row) -> Result<i64, DbError>` | Insert a row. Returns the new `rowid`. |
| `upsert` | `(table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>` | Insert or update on conflict. Returns the `rowid`. |
| `select` | `(table, columns, filter, order_by, limit) -> Result<Vec<Row>, DbError>` | Query rows. Pass `None` for `columns` to select `*`. |
| `update` | `(table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>` | Update matching rows. Returns the number of affected rows. |
| `delete` | `(table: &str, filter: DbFilter) -> Result<u64, DbError>` | Delete matching rows. Returns the number of deleted rows. |
| `count` | `(table: &str, filter: DbFilter) -> Result<i64, DbError>` | Count matching rows. |
| `last_insert_rowid` | `() -> Result<i64, DbError>` | Returns the `rowid` of the last inserted row. |
| `ping` | `() -> Result<(), DbError>` | Health-check the connection. |
| `shutdown` | `() -> Result<(), DbError>` | Gracefully close the client connection. |

All CRUD methods are `async`.

### Example

```rust
use ene_tool_db::{DbClient, DbSchema, DbTable, DbColumn, DbType, DbFilter, DbValue, Row};
use std::path::Path;

let client = DbClient::connect(Path::new("/run/ene/db.sock")).await?;

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

// Select
let rows = client.select(
    "fs_visited",
    None,  // SELECT *
    DbFilter::Eq {
        column: "path".into(),
        value: DbValue::Text("/home/user/notes.md".into()),
    },
    None,  // no ORDER BY
    Some(10),
).await?;

// Delete
let deleted = client.delete(
    "fs_visited",
    DbFilter::Eq {
        column: "id".into(),
        value: DbValue::Int(rowid),
    },
).await?;
println!("Deleted {deleted} rows");
```

---

## `DbError`

```rust
pub enum DbError {
    /// Low-level transport failure.
    Transport(io::Error),
    /// The server returned an application-level error.
    Server { code: DbErrorCode, message: String },
    /// The server sent a response that could not be decoded.
    UnexpectedResponse(String),
    /// The IPC connection was closed unexpectedly.
    ConnectionClosed,
}
```

### `DbErrorCode`

```rust
pub enum DbErrorCode {
    /// The tool does not have permission to access the requested resource
    /// (e.g. attempted to read or write a table outside its declared prefix).
    PermissionDenied,
    /// The specified table does not exist or is not declared.
    UnknownTable,
    /// The specified column does not exist in the table.
    UnknownColumn,
    /// A value's type does not match the column's declared type.
    TypeMismatch,
    /// The filter expression is invalid.
    InvalidFilter,
    /// An internal server error occurred.
    Internal,
}
```

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
    /// 64-bit signed integer.
    Integer,
    /// 64-bit IEEE floating point.
    Real,
    /// UTF-8 text string. (default)
    Text,
    /// Binary blob.
    Blob,
    /// Boolean (stored as INTEGER 0/1).
    Boolean,
}
```

### `DbValue`

```rust
pub enum DbValue {
    /// SQL NULL.
    Null,
    /// Boolean value.
    Bool(bool),
    /// 64-bit signed integer.
    Int(i64),
    /// 64-bit floating point.
    Float(f64),
    /// UTF-8 text string.
    Text(String),
    /// Binary blob.
    Blob(Vec<u8>),
}
```

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
    /// Matches all rows.
    Always,

    // Logical
    And(Vec<DbFilter>),
    Or(Vec<DbFilter>),
    Not(Box<DbFilter>),

    // Comparisons
    Eq  { column: String, value: DbValue },
    Ne  { column: String, value: DbValue },
    Gt  { column: String, value: DbValue },
    Lt  { column: String, value: DbValue },
    Ge  { column: String, value: DbValue },
    Le  { column: String, value: DbValue },

    // String
    Like { column: String, pattern: String },

    // Null checks
    IsNull    { column: String },
    IsNotNull { column: String },
}
```

### Example filters

```rust
// WHERE path LIKE '/home/%' AND visited_at > 1700000000
let filter = DbFilter::And(vec![
    DbFilter::Like {
        column: "path".into(),
        pattern: "/home/%".into(),
    },
    DbFilter::Gt {
        column: "visited_at".into(),
        value: DbValue::Int(1_700_000_000),
    },
]);

// WHERE id IS NOT NULL
let filter = DbFilter::IsNotNull {
    column: "id".into(),
};

// No filter (SELECT all rows)
let filter = DbFilter::Always;
```

---

## Prefix Access Control

The `DbIpcServer` in `ene-core` enforces the following rule for every request:

> A tool may only access tables whose name begins with `<tool_prefix>_`.

If a tool attempts to read or write a table outside its prefix, the server returns `DbError::Server { code: DbErrorCode::PermissionDenied, … }`.

This isolation is intentional: tools are separate, untrusted processes and must not be able to read or corrupt each other's state.

---

## Related Pages
