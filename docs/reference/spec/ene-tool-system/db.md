# Tool Database Proxy Specifications (`ene-tool-db`)

The `ene-tool-db` crate provides a database CRUD proxy client and communication protocol. It allows sandbox tool binaries to read and write database structures securely over a local socket connection to the host core.

---

## 1. Handshake & Auth Protocols

Tools establish connection using `db_socket` and `db_auth_token` provided as environment variables:

### 1. IPC Messages (`DbRequest` / `DbResponse`)

#### `DbRequest` (Tool → DB Server)
*   **`Handshake`**:
    -   `auth_token`: Ephemeral Blake3-hashed verification token.
    -   `tool_name`: Identifier of the tool.
*   **`DeclareSchema`**: Declares tables, columns, types, and indices (`DbSchema`).
*   **`Insert`**: Inserts a new row payload (`Row`).
*   **`Update`**: Modifies records matching a filter (`DbFilter`).
*   **`Delete`**: Deletes records matching a filter.
*   **`Select`**: Retrieves matching records based on `filters`, `order_by`, `limit`, and `offset`.

#### `DbResponse` (DB Server → Tool)
*   **`HandshakeAck`**: Handshake confirmed.
*   **`Success`**: Operation succeeded, returning row count or the inserted Row ID.
*   **`Rows`**: Returns matching records from a `Select` query.
*   **`Error`**: Returns error metadata (`DbErrorCode` and message).

---

## 2. Table-Name Prefix Isolation (Security Boundary)

To prevent tools from reading or overwriting core tables (e.g. `typed_memories`) or tables owned by other tool plugins, Ene enforces prefix isolation:
*   **Rule**: Every table declared in the tool's schema must start with its assigned `DbSchema::prefix` (e.g., `todo_`).
*   **Verification**: The host `DbIpcServer` validates every incoming `DbRequest` (Insert, Select, Update, etc.). If the target table does not match the tool's prefix, the server immediately rejects the query with `DbErrorCode::PermissionDenied` without executing any SQL commands.

---

## 3. Client Connection & Execution Methods (`client.rs`)

The `DbClient` implements the tool-side proxy API.

#### `connect`
*   **Signature**: `pub async fn connect(socket_path: &Path) -> Result<Self, DbError>`
*   **Description**: Connects to the database proxy socket using parameters read from the `ENE_TOOL_DB_AUTH_TOKEN` environment variable.

#### `connect_with_token`
*   **Signature**: `pub async fn connect_with_token(socket_path: &Path, token: &str) -> Result<Self, DbError>`
*   **Description**: Connects to the database proxy socket and performs the security handshake.

#### `socket_path`
*   **Signature**: `pub fn socket_path(&self) -> &Path`
*   **Description**: Returns the active socket path.

#### `reconnect`
*   **Signature**: `pub async fn reconnect(&mut self) -> Result<(), DbError>`
*   **Description**: Closes the current connection, establishes a new socket connection, and performs the handshake.

#### `send_request`
*   **Signature**: `async fn send_request(&mut self, req: &DbRequest) -> Result<DbResponse, DbError>`
*   **Description**: Helper method that serializes `DbRequest` to length-prefixed JSON Lines frames, writes it to the socket, and blocks until the corresponding `DbResponse` is read.

#### `check_error`
*   **Signature**: `fn check_error(resp: DbResponse) -> Result<DbResponse, DbError>`
*   **Description**: Translates database error responses carrying API-safe error codes into typed client errors.

#### `declare_schema`
*   **Signature**: `pub async fn declare_schema(&mut self, schema: DbSchema) -> Result<(Vec<String>, Vec<String>), DbError>`
*   **Description**: Submits the tool's schema definitions (tables, columns, and indices). Returns lists of created tables and indices.

#### `insert`
*   **Signature**: `pub async fn insert(&mut self, table: &str, row: Row) -> Result<i64, DbError>`
*   **Description**: Inserts a new row into the target table, returning the generated row ID.

#### `upsert`
*   **Signature**: `pub async fn upsert(&mut self, table: &str, row: Row, conflict_columns: &[&str]) -> Result<i64, DbError>`
*   **Description**: Inserts a row or updates existing fields on column conflict.

#### `select`
*   **Signature**: `pub async fn select(&mut self, table: &str, columns: &[&str], filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbError>`
*   **Description**: Retrieves records matching the query parameters and filters.

#### `update`
*   **Signature**: `pub async fn update(&mut self, table: &str, set: BTreeMap<String, DbValue>, filter: DbFilter) -> Result<u64, DbError>`
*   **Description**: Modifies records matching the filters. Returns the number of updated rows.

#### `delete`
*   **Signature**: `pub async fn delete(&mut self, table: &str, filter: DbFilter) -> Result<u64, DbError>`
*   **Description**: Deletes records matching the filters. Returns the number of deleted rows.

#### `count`
*   **Signature**: `pub async fn count(&mut self, table: &str, filter: DbFilter) -> Result<i64, DbError>`
*   **Description**: Returns the count of records matching the filters.

#### `last_insert_rowid`
*   **Signature**: `pub async fn last_insert_rowid(&mut self) -> Result<i64, DbError>`
*   **Description**: Returns the most recent row ID inserted via this connection.

#### `ping`
*   **Signature**: `pub async fn ping(&mut self) -> Result<(), DbError>`
*   **Description**: Sends a ping request to verify connection health.

#### `shutdown`
*   **Signature**: `pub async fn shutdown(&mut self) -> Result<(), DbError>`
*   **Description**: Closes the socket connection.

---

## 4. AST-Based SQL Generation

To prevent SQL injection, tools are barred from sending raw SQL strings to the database server.
*   **Structure**: Search filters are formatted as a `DbFilter` structure (defining column, operator like `Eq` or `Like`, and value `DbValue`).
*   **Parameter Binding**: The database server parses the structure and builds prepared SQL statements with parameter binding. Identifiers like table and column names are validated against `^[A-Za-z_][A-Za-z0-9_]*$` to block malicious injections.
