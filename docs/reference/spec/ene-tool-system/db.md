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

*   **Rule**:
    Every table declared in the tool's schema must start with its assigned `DbSchema::prefix` (e.g., `todo_`).
*   **Verification**:
    The host `DbIpcServer` validates every incoming `DbRequest` (Insert, Select, Update, etc.). If the target table does not match the tool's prefix, the server immediately rejects the query with `DbErrorCode::PermissionDenied` without executing any SQL commands.

---

## 3. AST-Based SQL Generation

To prevent SQL injection, tools are barred from sending raw SQL strings to the database server.

*   **Structure**:
    Search filters are formatted as a `DbFilter` structure (defining column, operator like `Eq` or `Like`, and value `DbValue`).
*   **Parameter Binding**:
    The database server parses the structure and builds prepared SQL statements with parameter binding. Identifiers like table and column names are validated against `^[A-Za-z_][A-Za-z0-9_]*$` to block malicious injections.
