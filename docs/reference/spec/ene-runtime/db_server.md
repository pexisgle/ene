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

---

## 2. Server & Client Lifecycle Functions

#### `new`
*   **Signature**: `pub const fn new(db: DatabaseConnection, socket_path: PathBuf, tool_name: String, prefix: String, auth_token: String) -> Self`
*   **Description**: Constructs a new `DbIpcServer` instance with the target sqlite-vec connection, socket path, tool identifier, and authorization credentials.

#### `run`
*   **Signature**: `pub async fn run(self) -> Result<(), DbServerError>`
*   **Process**:
    1.  Unix: Removes stale socket files via `remove_file` before binding.
    2.  Binds the `IpcListener` to the path or named pipe.
    3.  Unix: Strictly `chmod`s the socket to `0o600` immediately to prevent local privilege escalation.
    4.  Executes the infinite accept loop. In case of transient errors (e.g. `EMFILE`), the server backs off for 500ms and retries instead of crashing.
    5.  Spawns `handle_connection` tasks for each accepted client socket.

#### `handle_connection`
*   **Signature**: `async fn handle_connection(stream: ene_tool_proto::transport::IpcStream, db: DatabaseConnection, tool_name: String, prefix: String, auth_token: String) -> Result<(), DbServerError>`
*   **Process**:
    1.  **Connection Local State**:
        -   `last_rowid`: A cell (`Arc<Mutex<Option<i64>>>`) tracking the most recent `Insert` row ID. Because SeaORM uses a connection pool, executing a raw SQLite `last_insert_rowid()` query is racy. Storing it in memory per socket connection prevents cross-tool data races.
        -   `declared_tables` & `declared_columns`: Caches schemas declared by the tool.
    2.  **Length Prefix Parsing**:
        -   Reads a 4-byte little-endian message size header before each JSON payload.
        -   Caps message payloads at `64MB` to prevent out-of-memory denial-of-service vectors.
    3.  **Security Handshake**:
        -   The first message in the stream must be `DbRequest::Handshake { token }`.
        -   If the token does not match `auth_token`, the socket is closed immediately.

#### `send_response`
*   **Signature**: `async fn send_response(stream: &mut ene_tool_proto::transport::IpcStream, response: &DbResponse) -> Result<(), DbServerError>`
*   **Description**: Serializes the `DbResponse` object as JSON, pre-pends the 4-byte little-endian length prefix, and writes the frame to the client stream.

#### `handle_request`
*   **Signature**: `async fn handle_request(db: &DatabaseConnection, tool_name: &str, prefix: &str, declared_tables: &mut HashMap<String, DbTable>, declared_columns: &mut HashMap<String, HashSet<String>>, last_rowid: &Arc<std::sync::Mutex<Option<i64>>>, request: DbRequest) -> DbResponse`
*   **Description**: Deserializes the client query request, performs validation checks against declared schemas, sanitizes parameters, routes to the SQL engine, and maps output models.

---

## 3. Defense Mechanisms & Access Control

#### `validate_identifier`
*   **Signature**: `fn validate_identifier(name: &str) -> Result<(), DbServerError>`
*   **Description**: Ensures schema elements (tables, columns, indexes) are alpha-numeric (`^[A-Za-z_][A-Za-z0-9_]*$`) and fit within 64 characters to block SQL injection payloads in non-parameterizable identifiers.

#### `validate_table_access`
*   **Signature**: `fn validate_table_access(declared_tables: &HashMap<String, DbTable>, table: &str) -> Result<(), DbServerError>`
*   **Description**: Verifies if the target table name resides in the tool's declared schema registry.

#### `validate_row_columns`
*   **Signature**: `fn validate_row_columns(declared_columns: &HashMap<String, HashSet<String>>, table: &str, row: &Row) -> Result<(), DbServerError>`
*   **Description**: Validates that all keys inside insertion or update row maps correspond to declared schema fields.

#### `validate_select_columns`
*   **Signature**: `fn validate_select_columns(declared_columns: &HashMap<String, HashSet<String>>, table: &str, columns: &[String]) -> Result<(), DbServerError>`
*   **Description**: Confirms select target columns exist.

#### `validate_filter_columns`
*   **Signature**: `fn validate_filter_columns(declared_columns: &HashMap<String, HashSet<String>>, table: &str, filter: &DbFilter) -> Result<(), DbServerError>`
*   **Description**: Recursively scans evaluation structures inside filters for invalid column bindings.

#### `validate_order_by_columns`
*   **Signature**: `fn validate_order_by_columns(declared_columns: &HashMap<String, HashSet<String>>, table: &str, order_by: &[DbOrderBy]) -> Result<(), DbServerError>`
*   **Description**: Ensures order sorting fields match declared schemas.

#### `to_error_response`
*   **Signature**: `fn to_error_response(&self) -> DbResponse`
*   **Description**: Translates database internal exceptions or connection errors into structured JSON responses carrying API-safe diagnostics.

---

## 4. DDL & SQL Generation Functions

#### `handle_declare_schema`
*   **Signature**: `async fn handle_declare_schema(db: &DatabaseConnection, tool_name: &str, prefix: &str, declared_tables: &mut HashMap<String, DbTable>, declared_columns: &mut HashMap<String, HashSet<String>>, schema: DbSchema) -> Result<DbResponse, DbServerError>`
*   **Process**:
    1.  Validates table and column names via `validate_identifier`.
    2.  Forces namespaces: Prepends `<prefix>_` to table names.
    3.  Generates safe `CREATE TABLE` and `CREATE INDEX` strings.
    4.  Runs DDL transactions in the SQLite backend.
    5.  Records schemas in the system metadata tables (`__tool_schemas`).
    6.  Saves definitions in connection-local registries.

#### `build_create_table_sql`
*   **Signature**: `fn build_create_table_sql(table: &DbTable) -> String`
*   **Description**: Dynamically constructs SQLite-compatible `CREATE TABLE` strings with types, primary keys, and constraints.

#### `db_value_to_sql`
*   **Signature**: `fn db_value_to_sql(value: &DbValue) -> String`
*   **Description**: Serializes values into SQL-safe literals.

#### `build_create_index_sql`
*   **Signature**: `fn build_create_index_sql(index: &ene_tool_db::DbIndex) -> String`
*   **Description**: Generates `CREATE INDEX` queries.

#### `db_value_to_sea_value`
*   **Signature**: `fn db_value_to_sea_value(val: &DbValue) -> sea_orm::Value`
*   **Description**: Maps abstract tool database values to SeaORM-specific parameters.

---

## 5. DML Execution Functions

#### `handle_insert`
*   **Signature**: `async fn handle_insert(db: &DatabaseConnection, table: &str, row: Row) -> Result<i64, DbServerError>`
*   **Description**: Builds parameterised INSERT statements via SeaORM and returns the generated row ID.

#### `handle_upsert`
*   **Signature**: `async fn handle_upsert(db: &DatabaseConnection, table: &str, row: Row, conflict_columns: Vec<String>) -> Result<i64, DbServerError>`
*   **Description**: Formats SQLite `INSERT INTO ... ON CONFLICT DO UPDATE` statements for atomic store-upsert pipelines.

#### `handle_select`
*   **Signature**: `async fn handle_select(db: &DatabaseConnection, declared_tables: &HashMap<String, DbTable>, declared_columns: &HashMap<String, HashSet<String>>, table: &str, columns: Vec<String>, filter: DbFilter, order_by: Vec<DbOrderBy>, limit: Option<u64>) -> Result<Vec<Row>, DbServerError>`
*   **Description**: Compiles hybrid select transactions, binding filters, orders, limits, and returns row sets.

#### `handle_update`
*   **Signature**: `async fn handle_update(db: &DatabaseConnection, table: &str, set: Row, filter: DbFilter) -> Result<u64, DbServerError>`
*   **Description**: Translates target values and conditions into parameterised UPDATE statements. Returns row update counts.

#### `handle_delete`
*   **Signature**: `async fn handle_delete(db: &DatabaseConnection, table: &str, filter: DbFilter) -> Result<u64, DbServerError>`
*   **Description**: Runs DELETE queries matching conditions. Returns deleted row counts.

#### `handle_count`
*   **Signature**: `async fn handle_count(db: &DatabaseConnection, table: &str, filter: DbFilter) -> Result<i64, DbServerError>`
*   **Description**: Executes `SELECT COUNT(*)` statements.

#### `build_sea_query_filter`
*   **Signature**: `fn build_sea_query_filter(filter: &DbFilter) -> Result<Condition, DbServerError>`
*   **Description**: Iterates through structured and/or filter models, converting them to SeaORM `Condition` query builders.
