# IPC Wire Protocol & Sandbox Data Specifications (`ene-tool-proto`)

The `ene-tool-proto` crate defines the serialized wire contract, network stream transport, and sandbox configuration limits between Ene's core runtime and standalone tool processes.

---

## 1. Frame Protocol

### 1. Length-Prefixed JSON
All socket operations are framed with the following bytes:
*   **Header**: A 4-byte big-endian unsigned 32-bit integer (`u32`) indicating the length of the payload in bytes.
*   **Body**: A UTF-8 encoded JSON string.
*   **Size Limit**: Message bodies are capped at **64MB** (`MAX_MESSAGE_SIZE`) to prevent memory exhaustion and buffer overflows. Socket connections are terminated if this limit is exceeded.

---

## 2. IPC Request & Response (`IpcRequest` / `IpcResponse`)

### 1. `IpcRequest` (Core → Tool)
*   **`Handshake`**: Negotiates the protocol version and passes sandbox limits and tool settings.
*   **`ListTools`**: Requests all available action schemas (`ToolSpec`).
*   **`ListRagProfiles`**: Requests metadata profiles for RAG retrieval indexing.
*   **`CallTool`**: Requests tool execution, carrying the target `name` and JSON string `arguments`.
*   **`SetCallContext`**: Carries conversation and turn identifiers for caching and rollback snapshots.
*   **`Shutdown`**: Requests a graceful process shutdown.

### 2. `IpcResponse` (Tool → Core)
*   **`HandshakeAck`**: Returns the negotiated version.
*   **`Tools`**: Returns a list of supported `ToolSpec` definitions.
*   **`RagProfiles`**: Returns indexed RAG metadata profiles.
*   **`CallResult`**: Returns execution output: `Ok(String)` or `Err(ToolError)`.

---

## 3. Sandbox Configuration (`SandboxConfigData`)

Defines permission limits for tool processes, registered via `define_tool_config!`:

*   `enabled: bool`: Enables sandbox limits.
*   `allowed_directories: Vec<String>`: Directory paths allowed for read access (defaults to `.`).
*   `writable_directories: Vec<String>`: Directory paths allowed for write access.
*   `blocked_commands: Vec<String>`: Regular expression blocklists for shell commands (e.g., `rm -rf /`, `sudo`).
*   `max_read_bytes / max_write_bytes`: Size caps per read/write action (defaults to 50KB read / 1MB write).
*   `shell_timeout_ms`: Command execution timeout limit (defaults to 120,000 ms).
*   `db_socket / db_auth_token`: Connection credentials for the tool SQLite proxy server.

### Sanitize Helper (`sanitize()`)
*   To prevent tools from bypassing permissions by setting zero-values, the `sanitize()` method restores default blocklist regexes and overrides `0` limits with safe fallback values.
