# `DbIpcServer` / Tool Database Proxy Security Specification

The `DbIpcServer` implements a secure socket-based database proxy for external tool subprocesses. It blocks direct handles to the raw SQLite database file (`memory.db`) and rejects raw SQL string submissions, establishing a strict security boundary.

---

## 1. Server Data Structures

### `DbServerError` (Public / Error Enum)
Errors encountered during IPC socket listening, parsing, and query validation:
*   `Io(std::io::Error)`: Socket bind, accept, or read/write failures.
*   `Json(serde_json::Error)`: JSON Lines syntax parsing errors.
*   `Db(sea_orm::DbErr)`: Underlaying SeaORM SQL execution errors.
*   `PermissionDenied(String)`: Emitted for authorization failure, prefix bypass, or system table access.
*   `UnknownTable(String)`: Querying a table not declared in `DeclareSchema`.
*   `UnknownColumn { table, column }`: Referencing an undeclared column name.
*   `Internal(String)`: General server failures.

### `DbIpcServer` (Public / Server Handle)
One server task is spawned per active tool subprocess:
```rust
pub struct DbIpcServer {
    db: DatabaseConnection,
    socket_path: PathBuf,
    tool_name: String,
    prefix: String,
    auth_token: String,
}
```
*   `new(db: DatabaseConnection, socket_path: PathBuf, tool_name: String, prefix: String, auth_token: String) -> Self`: Constructor.
*   `async fn run(self) -> Result<(), DbServerError>`:
    1.  Unix: Removes stale socket files via `remove_file` before binding.
    2.  Binds the `IpcListener` to the path or named pipe.
    3.  Unix: Strictly `chmod`s the socket to `0o600` immediately to prevent local privilege escalation.
    4.  Executes the infinite accept loop. In case of transient errors (e.g. `EMFILE`), the server backs off for 500ms and retries instead of crashing.
    5.  Spawns `handle_connection` tasks for each accepted client socket.

---

## 2. Connection, Handshake & Lifecycles

### `handle_connection`
```rust
async fn handle_connection(
    stream: IpcStream,
    db: DatabaseConnection,
    tool_name: String,
    prefix: String,
    auth_token: String,
) -> Result<(), DbServerError>
```
1.  **Connection Local State**:
    -   `last_rowid`: A cell (`Arc<Mutex<Option<i64>>>`) tracking the most recent `Insert` row ID. Because SeaORM uses a connection pool, executing a raw SQLite `last_insert_rowid()` query is racy. Storing it in memory per socket connection prevents cross-tool data races.
    -   `declared_tables` & `declared_columns`: Caches schemas declared by the tool.
2.  **Length Prefix Parsing**:
    -   Reads a 4-byte little-endian message size header before each JSON payload.
    -   Caps message payloads at `64MB` to prevent out-of-memory denial-of-service vectors.
3.  **Security Handshake**:
    -   The first message in the stream must be `DbRequest::Handshake { token }`.
    -   If the token does not match `auth_token`, the socket is closed immediately.

---

## 3. Defense Mechanisms & Access Control

### 1. Identifier Validation (`validate_identifier`)
Because SQL table, column, and index names cannot be parameterized using standard query placeholders, the server sanitizes all schema identifiers:
*   Rejects empty values or strings longer than 64 characters.
*   Enforces matching `^[A-Za-z_][A-Za-z0-9_]*$`. Any violation throws a `PermissionDenied` error.

### 2. Prefix Validation (Namespace Isolation)
Every database operation is constrained by the tool's prefix (e.g. `fs_` or `web_`):
*   Enforced during `DeclareSchema` and subsequent `Select`, `Insert`, `Update`, `Delete`, and `Count` statements.
*   Queries to SQLite master metadata tables, other tool namespaces, or the schema storage table `__tool_schemas` are denied.

### 3. DDL Restriction
Tools cannot issue DDL (e.g. `CREATE TABLE`, `DROP`, `ALTER` SQL strings):
*   Structure declarations must go through `DbRequest::DeclareSchema`. The host generates and executes safe DDL internally.
*   No table deletion or structure alteration APIs exist.
