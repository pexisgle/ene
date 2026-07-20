# Tool Host Supervision & MCP Client Specifications (`ene-tool-host`)

The `ene-tool-host` crate supervises tool child processes, establishes socket communication channels, enforces name checks, manages crash retries, and integrates external MCP (Model Context Protocol) servers.

---

## 1. Process Supervisor (`ToolHostManager`)

The `ToolHostManager` orchestrates subprocess lifetimes based on entries in `config.json` under `tools.list.<name>`.

### 1. Process Spawning & Environment Isolation
When launching a tool process, the manager passes security settings as environment variables:
*   `ENE_DB_SOCKET`: Path to the tool database proxy Unix Domain Socket.
*   `ENE_DB_AUTH_TOKEN`: The Blake3-hashed handshake credential.
*   `ENE_TOOL_CONFIG`: Custom settings payload formatted as JSON.

### 2. Startup Validation
To prevent overlapping endpoints from confusing the LLM, name collision validation runs at startup. If a duplicated tool identifier is detected, the manager throws a fatal `ToolHostError::DuplicateToolName` and terminates the actor run.

---

## 2. Crash Resilience & Self-Healing

If a tool process crashes or drops its socket connection, `ToolHostManager` and `IpcToolRegistry` prevent session crashes by executing an automatic reconnect loop:

*   **Disconnection Detection**: EOF signals or socket write timeouts trigger the recovery loop.
*   **Exponential Backoff**:
    Tries reconnection up to 5 times before marking the tool offline:
    -   **Max Retries**: 5 attempts.
    -   **Initial Cooldown**: 500 ms.
    -   **Max Cooldown**: 30 seconds.
    -   **Backoff Multiplier**: 2.0x (e.g. 500ms → 1s → 2s → 4s ...).
*   If all 5 reconnect attempts fail, the tool status is transitioned to offline, and subsequent requests fail immediately.

---

## 3. Model Context Protocol (`McpToolRegistry`)

Ene provides native support for external MCP servers:
*   **Transports**:
    -   `stdio`: Launches an MCP subprocess, exchanging JSON-RPC payloads over stdin/stdout.
    -   `http`: Establishes connections to remote HTTP/SSE (Server-Sent Events) hosts.
*   **Schema Mapping**:
    Parses schemas returned by the MCP server and translates them into internal `ToolSpec` definitions.

---

## 4. Federated Registry (`CompositeToolRegistry`)

Exposes a unified query registry interface combining built-in actions, IPC subprocesses, and MCP servers.
*   **Routing**: Automatically maps `call_tool` invocations to the target subclass registry.
*   **Context-Aware Tool RAG**:
    Integrates with `ene-tool-rag` to dynamically select and inject only the most relevant tools into the active LLM context packet, saving token usage.
