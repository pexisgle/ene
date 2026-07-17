# `ene-tool-proto` — API Reference

> **Crate:** `ene-tool-proto`
> **Role:** IPC wire protocol for the Ene tool system — `ToolProvider`, `ToolSpec`, `IpcRequest`/`IpcResponse`, `ToolError`, and transport helpers.

---

## Overview

`ene-tool-proto` defines every type and helper that crosses the process boundary between the host runtime (`ene-runtime` / `ene-tool-host`) and the standalone tool binaries. Both sides of the IPC channel depend on this crate. It has no dependency on `ene-runtime`, so tool binaries can link it without pulling in the full runtime.

The crate has three responsibilities:

1. **The `ToolProvider` trait** — the interface every tool binary implements to describe and execute its tools.
2. **The wire protocol** — `IpcRequest` / `IpcResponse`, framed length-prefixed JSON over a Unix Domain Socket (Unix) or Named Pipe (Windows), plus the [`run_tool_server`](#run_tool_server) helper that turns a `ToolProvider` into a running IPC server.
3. **Shared metadata types** — `ToolSpec` (LLM-facing), `ToolRagProfile` (host/RAG, #137), `ToolName`, `ToolVersion`, `ToolCategory`, `KeywordSet`, `SideEffects`, and `ToolError`.

See also: [`ene-tool-host`](./ene-tool-host.md) for the host-side connection management, [`ene-tool-common`](./ene-tool-common.md) for the tool-side `ToolAction`/`ToolSpecArgs` traits, and [`ene-tool-derive`](./ene-tool-derive.md) for the proc-macros that generate `ToolSpec`s.

---

## Protocol Version

```rust
pub const IPC_PROTOCOL_VERSION: u32 = 4;
```

Both parties send their version in the `Handshake` / `HandshakeAck` messages. The server (`run_tool_server`) **strictly rejects** a mismatched version — it does not downgrade or negotiate — closing the connection with an `IpcResponse::Error`. Bump this constant only when the wire format changes in a backward-incompatible way (see [AGENTS.md §6 R3](../../../AGENTS.md)). Version **4** adds `ListRagProfiles` / `RagProfiles` for [`ToolRagProfile`](#toolragprofile) (#137). `ToolSpec` remains LLM-facing only (`name` / `description` / `parameters`).

---

## `ToolProvider` Trait

The interface each tool binary implements. The host-side `IpcToolRegistry` (in `ene-tool-host`) talks to a `ToolProvider` purely through IPC — this trait is the contract for what runs *inside* the tool process.

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_specs(&self) -> Vec<ToolSpec>;

    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> {
        Vec::new()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    fn set_session_id(&self, session_id: &str);

    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    fn approve_permission(&self, _request_id: &str) {}

    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    fn set_config(&self, _config: &serde_json::Value) {}

    fn get_config(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    fn config_schema(&self) -> Option<serde_json::Value> {
        None
    }
}
```

### Method Table

| Method | Required? | Default | Description |
|---|---|---|---|
| `list_specs(&self) -> Vec<ToolSpec>` | **Required** | — | Full metadata for every tool this provider exposes. Mega-tools return N specs, one per action (e.g. `filesystem.read`, `filesystem.write`, ...). |
| `list_rag_profiles(&self) -> Vec<ToolRagProfile>` | Optional | `Vec::new()` | Host/RAG metadata for Tool RAG indexing (#137). Prefer emitting from `#[derive(ToolSpec)]` / `ActionSetProvider`. |
| `call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>` | **Required** | — | Executes a tool by name with JSON-encoded `arguments`. `async`. |
| `set_session_id(&self, session_id: &str)` | **Required** | — | Sets the current session ID (used for undo tracking, session-scoped state, etc.). |
| `set_sandbox(&self, sandbox: &SandboxConfigData)` | Optional | no-op | Receives sandbox configuration (used by filesystem/shell tools). |
| `approve_permission(&self, request_id: &str)` | Optional | no-op | Approves a pending destructive-operation permission request by ID. |
| `allow_pattern(&self, action: &str, target_pattern: &str)` | Optional | no-op | Adds a session-wide permission allow pattern (action + target glob). |
| `set_config(&self, config: &serde_json::Value)` | Optional | no-op | Receives tool-specific configuration (called during `Initialize` or `SetMyConfig`). |
| `get_config(&self) -> serde_json::Value` | Optional | `Value::Null` | Returns the tool's current configuration. |
| `config_schema(&self) -> Option<serde_json::Value>` | Optional | `None` | Returns the JSON Schema for the configuration this tool accepts. |

`HostRegistry` (below) also implements `ToolProvider`, so a bundle of providers can be treated as a single provider.

---

## `run_tool_server`

```rust
pub async fn run_tool_server(provider: Box<dyn ToolProvider>) -> Result<(), ToolError>;
```

Starts a `ToolProvider` as an IPC server. This is **not generic** — it takes a boxed trait object, not `run_tool_server::<T>()`. Returns `ToolError` (not a boxed error trait object) so callers can `match` on the failure; socket I/O errors convert via `ToolError`'s `From<std::io::Error>` impl.

Behavior:

1. Reads the socket/pipe path from the `ENE_TOOL_SOCKET` environment variable, defaulting to `/tmp/ene-tool.sock` (Unix) or `\\.\pipe\ene-tool` (Windows).
2. Removes any stale socket file and binds a fresh listener; on Unix, `chmod`s the socket to `0600`.
3. Accepts connections in a loop, spawning a task per connection that reads `IpcRequest`s, dispatches them against the provider, and writes back `IpcResponse`s.
4. Shuts down cleanly when an `IpcRequest::Shutdown` is received (acknowledges it, then breaks the accept loop and removes the socket file).

---

## `HostRegistry`

```rust
#[derive(Default)]
pub struct HostRegistry { /* private fields */ }

impl HostRegistry {
    pub fn new() -> Self;
    pub fn add_provider(&mut self, provider: Box<dyn ToolProvider>);
    pub fn list_specs(&self) -> Vec<ToolSpec>;
    pub async fn call_tool(&self, name: &ToolName, arguments: &str) -> Result<String, ToolError>;
    pub fn set_session_id(&self, session_id: &str);
    pub fn set_sandbox(&self, sandbox: &SandboxConfigData);
}

impl ToolProvider for HostRegistry { /* ... */ }
```

A composite registry that aggregates multiple `ToolProvider`s and dispatches calls by tool name. Useful when bundling multiple providers into a single custom tool binary — a single provider is usually sufficient for standalone tool binaries.

### Method Table

| Method | Description |
|---|---|
| `new()` | Creates an empty registry. |
| `add_provider(provider)` | Registers a provider. First-registered provider wins on tool-name conflicts. Indexes every `ToolSpec::name` the provider exposes. |
| `list_specs()` | Returns all tool specs from all registered providers, concatenated. |
| `call_tool(name, arguments)` | Dispatches to the provider that registered `name`. Returns `ToolError::NotFound` if no provider owns that name. |
| `set_session_id(session_id)` | Broadcasts the session ID to every registered provider. |
| `set_sandbox(sandbox)` | Broadcasts the sandbox configuration to every registered provider. |

`HostRegistry` also implements `ToolProvider` itself: its `call_tool(&self, name: &str, ...)` trait-level method parses `name` with [`ToolName::try_new`](#toolname) and returns `ToolError::InvalidName` (rather than panicking) if the IPC-supplied string is malformed.

---

## Types

### `ToolSpec`

The structured, LLM-facing description of a single callable tool (`name`, `description`, `parameters` only — #135).

```rust
pub struct ToolSpec {
    pub name: ToolName,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema, auto-derived by schemars
}
```

| Method | Signature | Description |
|---|---|---|
| `new` | `fn new(name, description, parameters) -> Self` | Construct an LLM-facing tool spec. |

### `ToolRagProfile`

Host/RAG-only metadata for a callable tool (#137). Never passed to the LLM tool list — exchanged via `IpcResponse::RagProfiles` and consumed by `ene-tool-rag`.

```rust
pub struct ToolRagProfile {
    pub name: ToolName,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub category: ToolCategory,
    pub keywords: KeywordSet,
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub preconditions: Vec<String>,
    pub side_effects: SideEffects,
    pub related: Vec<ToolName>,
    pub version: ToolVersion,
}
```

| Method | Signature | Description |
|---|---|---|
| `from_tool_spec` | `fn from_tool_spec(spec: &ToolSpec) -> Self` | Synthesize a minimal profile (e.g. MCP tools). |
| `embedding_text` | `fn embedding_text(&self, field: EmbeddingField, parameters: Option<&Value>, example_index: Option<usize>) -> String` | Builds embedding text for a single index field. |

### `ToolName`

A validated, namespaced tool identifier — a newtype wrapper around `String`.

```rust
pub struct ToolName(/* private */ String);
```

Format: `"<namespace>.<action>"` for mega-tools (e.g. `"filesystem.read"`) or a bare `"<name>"` for individual tools (e.g. `"utility.get_current_time"`). Valid characters are ASCII alphanumeric, `_`, and `.` (no leading/trailing dots, never empty).

| Method | Signature | Description |
|---|---|---|
| `is_valid` | `fn is_valid(name: &str) -> bool` | Returns `true` if `name` is non-empty and contains only valid characters. |
| `new` | `fn new(name: impl Into<String>) -> Self` | Constructs a `ToolName`. **Panics** on invalid input — only use for trusted, compile-time-validated literals (e.g. `#[tool]` attribute names). |
| `try_new` | `fn try_new(name: impl Into<String>) -> Result<Self, String>` | Fallible constructor. **Use this for all untrusted input** — IPC tool names, MCP server tool names, config/env values, DB rows. |
| `namespace` | `fn namespace(&self) -> Option<&str>` | The namespace portion (`"filesystem.read"` → `Some("filesystem")`), or `None` for non-namespaced tools. |
| `action` | `fn action(&self) -> &str` | The action portion (`"filesystem.read"` → `"read"`). |
| `as_str` | `fn as_str(&self) -> &str` | Borrows the inner string. |
| `into_string` | `fn into_string(self) -> String` | Consumes `self`, returning the inner `String`. |

Also implements `Display`, `From<&str>` (panics on invalid input, like `new`), and `From<String>` (same).

### `ToolVersion`

```rust
pub struct ToolVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}
```

Semantic version used to invalidate the embedding cache only on semver-meaningful changes.

| Method | Signature | Description |
|---|---|---|
| `new` | `const fn new(major: u32, minor: u32, patch: u32) -> Self` | Constructs a version. |

Implements `Default` (`1.0.0`) and `Display` (`"{major}.{minor}.{patch}"`).

### `ToolCategory`

```rust
pub enum ToolCategory {
    Filesystem,
    Shell,
    Browser,
    App,
    WebSearch,
    WebFetch,
    Utility,
    Memory,
    Search,
    Meta,
}
```

Used for classification and RAG filtering. Category also drives `tools.rag.per_category_limits` via `ToolCategory::config_key()`.

| Method | Signature | Description |
|---|---|---|
| `label` | `fn label(&self) -> &'static str` | Human-readable label used in the embedding text for this category (e.g. `Filesystem` → `"filesystem_tools"`). |

### `KeywordSet`

```rust
pub struct KeywordSet {
    pub primary: Vec<String>,
    pub secondary: Vec<String>,
    pub domain: Vec<String>,
    pub negative: Vec<String>,
}
```

Structured keyword bag used by Tool RAG scoring. Each tier carries a different weight (`FieldWeights` in `ene-tool-host::rag`): `primary` ≈ `1.0`, `secondary` ≈ `0.6`, `domain` ≈ `0.3`, `negative` ≈ `-0.5` (soft penalty when query terms overlap).

| Method | Signature | Description |
|---|---|---|
| `primary_only` | `fn primary_only(primary: impl IntoIterator<Item = impl Into<String>>) -> Self` | Builds a `KeywordSet` with only `primary` keywords set. |
| `with_secondary` | `fn with_secondary(primary: ..., secondary: ...) -> Self` | Builds a `KeywordSet` with `primary` + `secondary` keywords set. |
| `is_empty` | `fn is_empty(&self) -> bool` | `true` if all four vecs are empty. |

### `SideEffects`

```rust
pub enum SideEffects {
    ReadOnly,
    FileSystem { mutates: bool },
    Network { external: bool },
    System { privileged: bool },
    Browser { mutates_dom: bool },
    Destructive,
    Idempotent,
}
```

What kind of side effect the tool has, used for safety analysis and sandbox filtering. `Default` is `ReadOnly`. Serialized with `#[serde(tag = "kind", rename_all = "snake_case")]`.

### `ToolExample`

```rust
pub struct ToolExample {
    pub description: String,
    pub input: serde_json::Value,
    pub output: Option<String>,
}
```

One example of the tool in use, shown to the LLM and used for example-based RAG embedding. When `output` is present, the example is treated as high-confidence and weighted higher in the RAG index.

### `EmbeddingField`

```rust
pub enum EmbeddingField {
    Summary,
    Description,
    Capability,
    Example,
    Negative,
}
```

Controls which subset of a [`ToolRagProfile`](#toolragprofile)'s text [`ToolRagProfile::embedding_text`](#toolragprofile) produces:

| Variant | Included text |
|---|---|
| `Summary` | `"{name}: {summary}"` |
| `Description` | description + keyword block + optional JSON Schema property summary |
| `Capability` | category label + summary + primary keywords |
| `Example` | one worked example (`example_index` selects the row) |
| `Negative` | `"{name} NOT: {negative keywords}"`, or `""` if empty |

| Method | Signature | Description |
|---|---|---|
| `as_str` | `fn as_str(&self) -> &'static str` | The string label persisted in the index (`"summary"`, `"description"`, `"capability"`, `"example"`, `"negative"`). |

### `SandboxConfigData`

```rust
pub struct SandboxConfigData {
    pub enabled: bool,                        // default: true
    pub allowed_directories: Vec<String>,     // default: ["."]
    pub writable_directories: Vec<String>,    // default: ["."]
    pub blocked_commands: Vec<String>,        // default: [rm -rf /, dd if=, mkfs, sudo, fork-bomb]
    pub max_read_bytes: usize,                // default: 50 * 1024
    pub max_write_bytes: usize,               // default: 1024 * 1024
    pub shell_timeout_ms: u64,                // default: 120_000
    pub max_shell_output_bytes: usize,        // default: 50 * 1024
    pub max_shell_output_lines: usize,        // default: 2000
    pub db_socket: Option<String>,            // default: None
    pub db_auth_token: Option<String>,        // default: None
}
```

A serializable, POD representation of the sandbox policy, sent during `IpcRequest::Handshake` (folded from former `Initialize` at v3) and generated via `ene_config::define_tool_config!`. Field notes:

- `db_socket` — path to the per-tool DB IPC socket (Unix Domain Socket). Tool binaries connect here to reach the core DB server for typed CRUD (see [`ene-tool-db`](./ene-tool-db.md)).
- `db_auth_token` — pre-shared token the tool binary must present in its very first `ene_tool_db::DbRequest::Handshake`. `None` disables DB access entirely for that tool.

Tool binaries should otherwise treat this struct's exact defaults as an implementation detail of the host and only rely on the field shapes above.

### `ToolConfigAccessor`

```rust
pub struct ToolConfigAccessor { /* private */ }

impl ToolConfigAccessor {
    pub fn new(initial_config: serde_json::Value) -> Self;
    pub async fn get<T: serde::de::DeserializeOwned>(&self) -> Result<T, ToolError>;
    pub async fn set<T: serde::Serialize>(&self, config: &T) -> Result<(), ToolError>;
}
```

A shared, `RwLock`-guarded holder for a tool's live JSON configuration, useful as a building block inside `ToolProvider::set_config`/`get_config` implementations.

| Method | Description |
|---|---|
| `new(initial_config)` | Wraps the given JSON value in an `Arc<RwLock<...>>`. |
| `get::<T>()` | Deserializes the stored JSON into `T`. Returns `ToolError::InvalidArguments` (not a silent default) if the stored value doesn't match `T`'s shape. |
| `set(config)` | Serializes `config` to JSON and stores it, returning `ToolError::InvalidArguments` if serialization fails. |

---

## `ToolError`

All tool failures are expressed as variants of `ToolError` (a type alias for `EneToolProtoError`). It is `Serialize`/`Deserialize` and crosses the IPC boundary inside `IpcResponse::CallResult`.

```rust
pub enum ToolError {
    // ── Generic ────────────────────────────────────────────────
    NotFound { tool_name: String },
    InvalidName { reason: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    Internal { message: String },
    Other { message: String },

    // ── Sandbox / Security ─────────────────────────────────────
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    CommandBlocked { command: String, reason: String },

    // ── Interactive (requires host action before retry) ─────────
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    UserInputRequired {
        request_id: String,
        prompt: UserInputPrompt,
    },

    // ── Transport / IPC ────────────────────────────────────────
    IpcTransport { message: String },
    IpcClient { message: String },

    // ── Timeout ────────────────────────────────────────────────
    Timeout { message: String },
    ShellTimeout { command: String, timeout_ms: u64 },

    // ── I/O ────────────────────────────────────────────────────
    IoError { message: String },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    ShellOutputTooLarge { size: u64, limit: u64 },
}
```

> **Note:** there is no `BrowserError`, `AppError`, or `WebSearchError` variant. Domain-specific tool failures (browser, app-automation, web-search) are reported through the generic variants above — typically `ExecutionFailed` or `Other` — not through dedicated per-domain variants.
>
> `InvalidName` is returned by `HostRegistry::call_tool` and other IPC entry points when a caller-supplied tool name fails `ToolName::try_new` validation, instead of panicking on malformed input.

`ToolError` implements `std::error::Error` and `From<std::io::Error>` (mapped to `IoError`).

### Interactive error flow

When a tool returns `PermissionRequired` or `UserInputRequired`, the host must:

1. Present the request to the user (or apply a policy).
2. Call `HostRegistry`/registry-level `approve_permission(request_id)`, or collect the user's answers.
3. Re-call `call_tool` with the same arguments.

---

## Interactive Tool Types

Used inside `ToolError::UserInputRequired` to collect structured answers from the user.

### `UserInputPrompt`

```rust
pub struct UserInputPrompt {
    pub items: Vec<QuestionItem>,
}
```

Implements `Display`, rendering each item as `"{index}. {question} (options: ...) [free text]"`.

### `QuestionItem`

```rust
pub struct QuestionItem {
    pub question: String,
    pub options: Vec<String>,
    pub allow_free_text: bool,
}
```

If `options` is non-empty, the user must pick from that list unless `allow_free_text` is `true`.

### `MultiAnswer`

```rust
pub enum MultiAnswer {
    Selected { option: String },
    Answer { text: String },
    Skip,
}
```

Returned as a `Vec<MultiAnswer>`, one entry per `QuestionItem`, in the same order as `UserInputPrompt::items`.

---

## `IpcRequest`

Messages sent from the **core** (`ene-runtime` / `ene-tool-host`) **to** the tool binary.

```rust
pub enum IpcRequest {
    Handshake {
        version: u32,
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    },
    ListTools,
    ListRagProfiles,
    GetConfigSchema,
    CallTool { name: String, arguments: String },
    SetCallContext { conversation_id: String, turn_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    Shutdown,
}
```

| Variant | Purpose |
|---|---|
| `Handshake { version, sandbox, tool_config }` | Negotiate protocol version and push sandbox + tool config (v3+). |
| `ListTools` | Request the tool's LLM-facing `ToolSpec` list. |
| `ListRagProfiles` | Request host/RAG `ToolRagProfile` list (#137 / IPC v4). |
| `GetConfigSchema` | Request the tool's configuration JSON Schema (#150 exception). |
| `CallTool { name, arguments }` | Invoke a tool by name with JSON arguments. |
| `SetCallContext { conversation_id, turn_id }` | Propagate conversation + turn identifiers. |
| `ApprovePermission { request_id }` | Grant a pending permission request. |
| `AllowPattern { action, target_pattern }` | Add a pattern to the sandbox allow-list. |
| `Shutdown` | Graceful shutdown. |

---

## `IpcResponse`

Messages sent from the **tool binary** back to the **core** (`ene-runtime` / `ene-tool-host`).

```rust
pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    RagProfiles { profiles: Vec<ToolRagProfile> },
    ConfigSchema { schema: Option<serde_json::Value> },
    CallResult { result: Result<String, ToolError> },
    Error { message: String },
}
```

| Variant | Purpose |
|---|---|
| `HandshakeAck { version }` | Acknowledge the Handshake with the negotiated version. |
| `Ack` | Generic acknowledgment (for `SetCallContext`, permissions, etc.). |
| `Tools { tools }` | Response to `ListTools`. |
| `RagProfiles { profiles }` | Response to `ListRagProfiles` (#137). |
| `ConfigSchema { schema }` | Response to `GetConfigSchema`. |
| `CallResult { result }` | Response to `CallTool`. |
| `Error { message }` | Unrecoverable tool-side error (outside a specific call), e.g. a handshake version mismatch. |

### Message sequence diagram

```text
Host                          Tool
 │                             │
 │── Handshake ───────────────▶│
 │◀── HandshakeAck ────────────│
 │── Initialize ──────────────▶│
 │◀── Ack ─────────────────────│
 │── ListTools ───────────────▶│
 │◀── Tools([...]) ────────────│
 │                             │
 │── CallTool(name, args) ────▶│
 │◀── CallResult(Ok(str)) ─────│   happy path
 │                             │
 │── CallTool(name, args) ────▶│
 │◀── CallResult(Err(PermissionRequired{...}))
 │    [host approves]          │
 │── ApprovePermission(id) ───▶│
 │◀── Ack ─────────────────────│
 │── CallTool(name, args) ────▶│   retry
 │◀── CallResult(Ok(str)) ─────│
```

---

## Transport

### `IpcStream`

A cross-platform, framed byte stream implementing `AsyncRead` + `AsyncWrite`:

- **Unix** — wraps `tokio::net::UnixStream` (Unix Domain Socket, `AF_UNIX`).
- **Windows** — wraps `NamedPipeServer` (server-end) or `NamedPipeClient` (client-end), i.e. `\\.\pipe\...`.

| Method | Signature | Description |
|---|---|---|
| `connect` | `async fn connect(path: &Path) -> io::Result<Self>` | Connects to a listening IPC endpoint (platform-appropriate). |

### `IpcListener`

Cross-platform IPC listener.

| Method | Signature | Description |
|---|---|---|
| `bind` | `fn bind(path: &Path) -> io::Result<Self>` | Binds to an IPC endpoint. On Unix, wraps `UnixListener`. On Windows, creates the first named-pipe instance. |
| `accept` | `async fn accept(&mut self) -> io::Result<IpcStream>` | Accepts a new connection. On Windows, transparently recreates the next pipe instance after each accept. |

### `cleanup_path`

```rust
pub fn cleanup_path(path: &Path);
```

Removes the socket file on Unix; no-op on Windows (named pipes are not filesystem objects).

### Generic wire helpers

These four functions are generic over `AsyncReadExt`/`AsyncWriteExt`, not tied to `IpcStream` — they also work directly against `tokio::io::duplex` streams (as used in the crate's own tests) or any other async byte stream:

```rust
/// Reads an `IpcRequest` as 4-byte length-prefixed JSON.
/// Returns `Ok(None)` on `UnexpectedEof` (connection closed).
pub async fn read_ipc_request<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcRequest>, ToolError>;

/// Writes an `IpcRequest` as 4-byte length-prefixed JSON.
pub async fn write_ipc_request<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    req: &IpcRequest,
) -> Result<(), ToolError>;

/// Reads an `IpcResponse` as 4-byte length-prefixed JSON.
/// Returns `Ok(None)` on `UnexpectedEof` (connection closed).
pub async fn read_ipc_response<R: AsyncReadExt + Unpin>(
    reader: &mut R,
) -> Result<Option<IpcResponse>, ToolError>;

/// Writes an `IpcResponse` as 4-byte length-prefixed JSON.
pub async fn write_ipc_response<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    resp: &IpcResponse,
) -> Result<(), ToolError>;
```

Framing format: `[u32 little-endian length][JSON payload]`. Maximum message size is 64 MB (`MAX_MESSAGE_SIZE`, private to `ene_tool_proto::ipc`); requests/responses larger than that are rejected with `ToolError::IpcTransport`.

---

## Errors

All fallible operations in this crate report through [`ToolError`](#toolerror) (alias `EneToolProtoError`). There is no separate "transport error" type — I/O failures on the wire helpers are converted into `ToolError::IoError` via `From<std::io::Error>`, and malformed JSON is reported as `ToolError::InvalidArguments`.

---

## Usage

### Implementing a `ToolProvider`

```rust,no_run
use async_trait::async_trait;
use ene_tool_proto::{
    KeywordSet, SideEffects, ToolCategory, ToolError, ToolName, ToolProvider, ToolSpec,
    ToolVersion, run_tool_server,
};

struct MyTool;

#[async_trait]
impl ToolProvider for MyTool {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: ToolName::new("hello"),
            version: ToolVersion::new(1, 0, 0),
            display_name: "Hello".into(),
            summary: "Greets the user".into(),
            description: "Greets the user with a personalised message.".into(),
            category: ToolCategory::Utility,
            keywords: KeywordSet::primary_only(["greet", "hello", "greeting"]),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Name to greet"}
                },
                "required": ["name"]
            }),
            examples: vec![],
            caveats: vec![],
            side_effects: SideEffects::ReadOnly,
            preconditions: vec![],
            related: vec![],
        }]
    }

    async fn call_tool(&self, _name: &str, args: &str) -> Result<String, ToolError> {
        let v: serde_json::Value = serde_json::from_str(args)
            .map_err(|e| ToolError::InvalidArguments { message: e.to_string() })?;
        Ok(format!("Hello, {}!", v["name"].as_str().unwrap_or("world")))
    }

    fn set_session_id(&self, _sid: &str) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // NOT run_tool_server::<MyTool>() — the function takes a boxed trait object.
    run_tool_server(Box::new(MyTool)).await?;
    Ok(())
}
```

### Bundling multiple providers with `HostRegistry`

```rust,no_run
use ene_tool_proto::{HostRegistry, ToolProvider, run_tool_server};

fn build_registry(a: Box<dyn ToolProvider>, b: Box<dyn ToolProvider>) -> HostRegistry {
    let mut registry = HostRegistry::new();
    registry.add_provider(a);
    registry.add_provider(b);
    registry
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let a: Box<dyn ToolProvider> = unimplemented!();
    let b: Box<dyn ToolProvider> = unimplemented!();
    let registry = build_registry(a, b);
    run_tool_server(Box::new(registry)).await?;
    Ok(())
}
```

### Validating an untrusted tool name

```rust,no_run
use ene_tool_proto::{ToolError, ToolName};

fn handle_ipc_call_tool(raw_name: &str) -> Result<ToolName, ToolError> {
    // Untrusted input (came off the wire) — use try_new, not new.
    ToolName::try_new(raw_name).map_err(|reason| ToolError::InvalidName { reason })
}
```

### Building a `ToolRagProfile` from a slim `ToolSpec`

```rust,no_run
use ene_tool_proto::{ToolName, ToolRagProfile, ToolSpec};

let spec = ToolSpec::new(ToolName::new("mcp.hello"), "Say hello", serde_json::json!({}));
let profile = ToolRagProfile::from_tool_spec(&spec);
assert_eq!(profile.summary, "Say hello");
```

### Computing RAG embedding text

```rust,no_run
use ene_tool_proto::types::EmbeddingField;
use ene_tool_proto::ToolRagProfile;

fn summary_embedding(profile: &ToolRagProfile) -> String {
    profile.embedding_text(EmbeddingField::Summary, None, None)
}
```

---

## Related Pages

- [`ene-tool-host`](./ene-tool-host.md) — Host-side lifecycle and registry
- [`ene-tool-common`](./ene-tool-common.md) — Tool-side `ToolAction`/`ToolSpecArgs` traits
- [`ene-tool-derive`](./ene-tool-derive.md) — Proc-macros for generating `ToolSpec`
- [`ene-tool-db`](./ene-tool-db.md) — Per-tool database IPC that rides on `SandboxConfigData::db_socket`
