# IPC Wire Protocol & Sandbox Data Specifications (`ene-tool-proto`)

The `ene-tool-proto` crate defines the serialized wire contract, network stream transport, and sandbox configuration limits between Ene's core runtime and standalone tool processes.

---

## 1. Frame Protocol & Transport (`transport.rs`)

All socket operations are framed using length-prefixed JSON Lines.

#### `IpcStream::connect`
*   **Signature**: `pub async fn connect(path: &Path) -> io::Result<Self>`
*   **Description**: Connects to the local database socket (Unix Domain Sockets on Linux/macOS, Named Pipes on Windows).

#### `poll_read` / `poll_write` / `poll_flush` / `poll_shutdown`
*   **Description**: Standard async I/O implementations for `IpcStream`.

#### `IpcListener::bind`
*   **Signature**: `pub fn bind(path: &Path) -> io::Result<Self>`
*   **Description**: Binds a socket listener.

#### `IpcListener::accept`
*   **Signature**: `pub async fn accept(&mut self) -> io::Result<IpcStream>`
*   **Description**: Awaits incoming tool client connections.

#### `cleanup_path`
*   **Signature**: `pub fn cleanup_path(path: &Path)`
*   **Description**: Removes stale socket files.

---

## 2. IPC Request & Response Serialization (`ipc.rs`)

#### `read_ipc_request`
*   **Signature**: `pub async fn read_ipc_request<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<IpcRequest>, ToolError>`
*   **Description**: Reads a 4-byte big-endian `u32` header, verifies it is under the 64MB limit (`MAX_MESSAGE_SIZE`), reads the JSON body, and deserializes it to `IpcRequest`.

#### `write_ipc_request`
*   **Signature**: `pub async fn write_ipc_request<W: AsyncWriteExt + Unpin>(writer: &mut W, req: &IpcRequest) -> Result<(), ToolError>`
*   **Description**: Serializes `IpcRequest` to JSON, prepends the length header, and writes the frame.

#### `read_ipc_response`
*   **Signature**: `pub async fn read_ipc_response<R: AsyncReadExt + Unpin>(reader: &mut R) -> Result<Option<IpcResponse>, ToolError>`
*   **Description**: Reads and deserializes `IpcResponse` payloads.

#### `write_ipc_response`
*   **Signature**: `pub async fn write_ipc_response<W: AsyncWriteExt + Unpin>(writer: &mut W, resp: &IpcResponse) -> Result<(), ToolError>`
*   **Description**: Serializes and writes `IpcResponse` payloads.

#### `IpcConfig::new`
*   **Signature**: `pub fn new(initial_config: serde_json::Value) -> Self`
*   **Description**: Creates a new config handle.

#### `IpcConfig::get` / `set`
*   **Signature**: `pub async fn get<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolError>` (same patterns for set)
*   **Description**: Safely extracts and updates configurations.

---

## 3. Tool Server & Dispatch Logic (`server.rs`)

#### `run_tool_server`
*   **Signature**: `pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), ToolError>`
*   **Process**:
    1.  Resolves socket paths from environment variables.
    2.  Establishes connections to the host database socket.
    3.  Enters the loop: reads requests, dispatches them via `dispatch`, and writes back response frames.

#### `dispatch`
*   **Signature**: `async fn dispatch(provider: &dyn ToolProvider, req: &IpcRequest) -> IpcResponse`
*   **Description**: Handles incoming requests, calling providers to retrieve schemas or execute target tools.

---

## 4. Metadata Models & Profiles (`types.rs`)

#### `ToolName::try_new`
*   **Signature**: `pub fn try_new(name: impl Into<String>) -> Result<Self, String>`
*   **Description**: Parses and validates names. Verifies they match the format `<namespace>.<action>` and contain no invalid characters.

#### `ToolName::namespace` / `action`
*   **Signature**: `pub fn namespace(&self) -> Option<&str>` (same pattern for actions)
*   **Description**: Returns namespace or action name components.

#### `ToolVersion::new`
*   **Signature**: `pub const fn new(major: u32, minor: u32, patch: u32) -> Self`
*   **Description**: Constructs a tool version.

#### `KeywordSet::primary_only` / `with_secondary`
*   **Signature**: `pub fn primary_only(primary: impl IntoIterator<Item = impl Into<String>>) -> Self` (same for secondary)
*   **Description**: Configures keyword matching arrays for Tool RAG.

#### `ToolRagProfile::from_tool_spec`
*   **Signature**: `pub fn from_tool_spec(spec: &ToolSpec) -> Self`
*   **Description**: Builds RAG retrieval profiles.

#### `ToolRagProfile::embedding_text`
*   **Signature**: `pub fn embedding_text(&self, field: EmbeddingField, parameters: Option<&serde_json::Value>, example_index: Option<usize>) -> String`
*   **Description**: Formats tool schemas and descriptions into structured text for vector embeddings.

---

## 5. Tool Providers Host Registry (`host_registry.rs`)

#### `HostRegistry::new`
*   **Signature**: `pub fn new() -> Self`
*   **Description**: Constructs an empty registry.

#### `HostRegistry::try_add_provider`
*   **Signature**: `pub fn try_add_provider(&mut self, provider: Box<dyn ToolProvider>) -> Result<(), ToolError>`
*   **Description**: Registers a provider. Returns an error on name collisions.

#### `HostRegistry::list_specs` / `list_rag_profiles`
*   **Signature**: `pub fn list_specs(&self) -> Vec<ToolSpec>`
*   **Description**: Lists all registered specifications.

#### `HostRegistry::call_tool`
*   **Signature**: `pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError>`
*   **Description**: Routes calls to the provider matching the namespace.

#### `HostRegistry::set_call_context` / `set_sandbox`
*   **Signature**: `pub fn set_call_context(&self, ctx: &CallContext)`
*   **Description**: Sets current context properties across all registered providers.

---

## 6. Sandbox Configuration (`SandboxConfigData`)

Defines permission limits for tool processes:

#### `SandboxConfigData::sanitize`
*   **Signature**: `pub fn sanitize(&mut self)`
*   **Description**: Restores default command blocklists and overrides zero limits with safe fallback values to prevent security bypasses.
