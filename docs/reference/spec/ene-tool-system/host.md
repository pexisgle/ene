# Tool Host Supervision & MCP Client Specifications (`ene-tool-host`)

The `ene-tool-host` crate supervises tool child processes, establishes socket communication channels, enforces name checks, manages crash retries, and integrates external MCP (Model Context Protocol) servers.

---

## 1. Configurations & Versioning Helpers (`config.rs` & `tools/mod.rs`)

#### `deserialize_config`
*   **Signature**: `pub fn deserialize_config<T>(&self) -> Result<T, serde_json::Error> where T: DeserializeOwned`
*   **Description**: Helper method that deserializes custom JSON configuration blocks into structured configurations.

#### `compute_tool_version_hash`
*   **Signature**: `pub fn compute_tool_version_hash(tool: &ene_tool_proto::ToolSpec) -> String`
*   **Description**: Computes a stable Blake3 hash from a tool's specifications (input properties, version numbers, keywords) to track changes and refresh registries.

---

## 2. Process Supervisor (`ToolHostManager`)

The `ToolHostManager` orchestrates subprocess lifetimes based on entries in `config.json` under `tools.list.<name>`.

#### `start`
*   **Signature**: `pub async fn start(config: &EneConfig, mut db_tokens: std::collections::HashMap<String, String>) -> Result<Self, ToolHostError>`
*   **Description**: Scans configurations and initializes subprocesses.

#### `start_full`
*   **Signature**: `pub async fn start_full(config: &EneConfig, db_tokens: std::collections::HashMap<String, String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>`
*   **Description**: Starts subprocesses and wraps them into a unified, thread-safe `CompositeToolRegistry`.

#### `try_add_registry`
*   **Signature**: `pub fn try_add_registry(&mut self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError>`
*   **Description**: Adds a new registry layer to the manager. Returns `DuplicateToolName` if tool names overlap.

#### `into_registry`
*   **Signature**: `pub fn into_registry(self) -> Arc<dyn ToolRegistry>`
*   **Description**: Returns the unified `CompositeToolRegistry` handle.

#### `start_tool`
*   **Signature**: `async fn start_tool(name: &str, sandbox: &ene_tool_proto::SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64, db_token: Option<String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>`
*   **Process**:
    1.  Locates the executable binary via `find_tool_binary`.
    2.  Creates temporary IPC paths and binds listener sockets.
    3.  Sets environment parameters (including security credentials `ENE_DB_AUTH_TOKEN` and socket paths).
    4.  Spawns the child subprocess.
    5.  Awaits socket connection handshakes with retry parameters.
    6.  Returns the instantiated `IpcToolRegistry`.

#### `find_tool_binary`
*   **Signature**: `fn find_tool_binary(name: &str) -> Option<PathBuf>`
*   **Description**: Searches builtin asset folders and user binary directories for executable tool binaries matching the target name.

#### `is_alive`
*   **Signature**: `fn is_alive(&mut self) -> bool`
*   **Description**: Checks if the supervisor's child process handle is active.

#### `restart`
*   **Signature**: `fn restart(&mut self) -> Result<(), ToolHostError>`
*   **Description**: Aborts zombie processes and spawns a fresh subprocess.

#### `delay_for_restart`
*   **Signature**: `fn delay_for_restart(restart_count: usize) -> Duration`
*   **Description**: Computes backoffs for process crashes. Caps maximum cooldowns at 30 seconds.

---

## 3. IPC Client Proxy Registry (`ipc_registry.rs`)

Manages communication with active subprocesses.

#### `new` (for IpcToolRegistry)
*   **Signature**: `pub async fn new(socket_path: PathBuf, sandbox: SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64) -> Result<Self, ToolHostError>`
*   **Description**: Constructs a registry link monitoring a child socket.

#### `connect_with_retry`
*   **Signature**: `pub(crate) async fn connect_with_retry(socket_path: &Path, sandbox: &ene_tool_proto::SandboxConfigData, tool_config: Option<serde_json::Value>, max_retries: u32, delay_ms: u64, timeout_ms: u64) -> Result<IpcToolRegistry, ToolError>` (and `connect_with_retry` at line 99)
*   **Description**: Attempts to connect to the target socket, retrying with exponential backoff on transient errors.

#### `do_request`
*   **Signature**: `async fn do_request(&self, req: IpcRequest) -> Result<IpcResponse, ToolHostError>`
*   **Description**: Writes requests to the IPC socket and reads responses.

#### `do_refresh_tools_with_stream`
*   **Signature**: `async fn do_refresh_tools_with_stream(&self, stream: &mut IpcStream) -> Result<(), ToolHostError>`
*   **Description**: Queries the socket stream for the tool's schemas (`IpcRequest::ListTools`), validating definitions.

#### `do_refresh_tools` / `refresh_tools`
*   **Signature**: `pub async fn refresh_tools(&self) -> Result<(), ToolHostError>`
*   **Description**: Refreshes cached tool schemas.

#### `ensure_connected`
*   **Signature**: `async fn ensure_connected(&self) -> Result<(), ToolHostError>`
*   **Description**: Verifies connection health, triggering reconnect loops if disconnected.

#### `send_with_reconnect`
*   **Signature**: `async fn send_with_reconnect(&self, req: IpcRequest) -> Result<IpcResponse, ToolHostError>`
*   **Description**: Sends requests, attempting to reconnect if the socket is disconnected.

---

## 4. Model Context Protocol (`McpToolRegistry`)

Provides support for external MCP servers.

#### `connect_stdio`
*   **Signature**: `pub async fn connect_stdio(&self, name: &str, command: &str, args: &[&str]) -> Result<(), ToolHostError>`
*   **Description**: Launches an MCP server subprocess, setting up JSON-RPC communication over stdin/stdout.

---

## 5. Federated Registry (`CompositeToolRegistry`)

#### `try_new` / `new`
*   **Signature**: `pub fn try_new(registries: Vec<Arc<dyn ToolRegistry>>) -> Result<Self, ToolHostError>`
*   **Description**: Combines multiple registries into a single interface. Verifies that tool names do not overlap.

#### `try_add_registry`
*   **Signature**: `pub fn try_add_registry(&self, registry: Arc<dyn ToolRegistry>) -> Result<(), ToolHostError>`
*   **Description**: Dynamic registration helper.

#### `call_tool`
*   **Signature**: `async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>`
*   **Description**: Dispatches tool calls to the matching registry layer based on name prefix namespaces.
