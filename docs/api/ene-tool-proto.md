# `ene-tool-proto`

> IPC wire protocol for the Ene tool system — `ToolSpec`, `IpcRequest`/`IpcResponse`, `ToolError`, and transport helpers.

`ene-tool-proto` defines every type that crosses the process boundary between `ene-tool-host` (the host) and the standalone tool binaries. Both sides of the IPC channel depend on this crate. It has no dependency on `ene-core` and can be imported by tool binaries without pulling in the full runtime.

See also: [`ene-tool-host`](ene-tool-host.md) for the host-side connection management, [`ene-tool-common`](ene-tool-common.md) for the tool-side `ToolAction` trait.

---

## Protocol Version

```rust
pub const IPC_PROTOCOL_VERSION: u32 = 1;
```

Both parties send their version in the `Handshake` / `HandshakeAck` messages. A mismatch causes the connection to be terminated. Bump this constant only when the wire format changes in a backward-incompatible way (see [AGENTS.md §4 R3](../../AGENTS.md)).

---

## `ToolSpec`

Describes a single callable tool. This is the primary metadata type used by the Tool RAG pipeline and passed to LLMs as part of the tool list.

```rust
pub struct ToolSpec {
    pub name: ToolName,
    pub version: ToolVersion,
    pub display_name: String,
    pub summary: String,
    pub description: String,
    pub category: ToolCategory,
    pub keywords: KeywordSet,
    pub parameters: serde_json::Value,  // JSON Schema object
    pub examples: serde_json::Value,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
    pub related: Vec<ToolName>,
}
```

### Supporting types

#### `KeywordSet`

```rust
pub struct KeywordSet {
    /// High-signal terms directly describing what the tool does.
    pub primary: Vec<String>,
    /// Supporting or contextual terms.
    pub secondary: Vec<String>,
    /// Domain tags (e.g. "filesystem", "web", "shell").
    pub domain: Vec<String>,
    /// Terms that indicate when this tool should NOT be used.
    pub negative: Vec<String>,
}
```

The `negative` set is used by the RAG pipeline to down-rank tools when query terms overlap with negative keywords.

#### `SideEffects`

```rust
pub enum SideEffects {
    None,
    ReadOnly,
    Writes,
    Network,
    Destructive,
}
```

#### `ToolName` / `ToolVersion` / `ToolCategory`

Newtype wrappers around `String`. `ToolName` follows the convention `<namespace>.<action>` (e.g. `fs.read_file`).

---

## `ToolError`

All tool failures are expressed as variants of `ToolError`. It is `Serialize`/`Deserialize` and crosses the IPC boundary inside `IpcResponse::CallResult`.

```rust
pub enum ToolError {
    // ── Generic ────────────────────────────────────────────────
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    Internal { message: String },
    Other { message: String },

    // ── Sandbox / Security ─────────────────────────────────────
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    CommandBlocked { command: String, reason: String },

    // ── Interactive (requires host action before retry) ─────────
    /// Tool requires explicit user/host permission.
    PermissionRequired {
        request_id: String,
        action: String,
        target: String,
        description: String,
    },
    /// Tool requires user answers to continue.
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
    ShellOutputTooLarge { size: usize, limit: usize },

    // ── Domain-specific ────────────────────────────────────────
    BrowserError { message: String },
    AppError { message: String },
    WebSearchError { message: String },
}
```

### Interactive error flow

When a tool returns `PermissionRequired` or `UserInputRequired`, the host must:

1. Present the request to the user (or apply a policy).
2. Call `ToolRegistry::approve_permission(request_id)` or collect answers.
3. Re-call `call_tool` with the same arguments.

---

## `IpcRequest`

Messages sent from the **host** (`ene-tool-host`) **to** the tool binary.

```rust
pub enum IpcRequest {
    /// Handshake to negotiate protocol version. Must be the first message.
    Handshake { version: u32 },
    /// Provide sandbox policy and per-tool config.
    Initialize {
        sandbox: SandboxConfigData,
        tool_config: Option<serde_json::Value>,
    },
    /// Request the tool's full metadata list.
    ListTools,
    /// Request per-action metadata (for mega-tool embedding).
    ListActionSpecs,
    /// Request the tool's configuration JSON Schema.
    GetConfigSchema,
    /// Invoke a tool by name with JSON arguments.
    CallTool { name: String, arguments: String },
    /// Propagate the active session ID.
    SetSessionId { session_id: String },
    /// Grant a pending permission request.
    ApprovePermission { request_id: String },
    /// Add a pattern to the sandbox allow-list.
    AllowPattern { action: String, target_pattern: String },
    /// Get the tool's configuration.
    GetMyConfig,
    /// Replace the tool's configuration.
    SetMyConfig(serde_json::Value),
    /// Health-check ping.
    Ping,
    /// Graceful shutdown.
    Shutdown,
}
```

---

## `IpcResponse`

Messages sent from the **tool binary** back to the host.

```rust
pub enum IpcResponse {
    /// Acknowledge the Handshake with the negotiated version.
    HandshakeAck { version: u32 },
    /// Generic acknowledgment (for Initialize, `SetSessionId`, etc.).
    Ack,
    /// Response to ListTools.
    Tools { tools: Vec<ToolSpec> },
    /// Response to ListActionSpecs. For mega-tools, one entry per action.
    ActionSpecs { specs: Vec<ActionSpec> },
    /// Response to GetConfigSchema.
    ConfigSchema { schema: Option<serde_json::Value> },
    /// Response to CallTool.
    CallResult { result: Result<String, ToolError> },
    /// Response to GetMyConfig.
    MyConfig(serde_json::Value),
    /// Pong response to Ping.
    Pong,
    /// Unrecoverable tool-side error (outside a specific call).
    Error { message: String },
}
```

### Message sequence diagram

```
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

## Interactive Tool Types

Used in `ToolError::UserInputRequired` to collect structured answers from the user.

### `UserInputPrompt`

```rust
pub struct UserInputPrompt {
    pub items: Vec<QuestionItem>,
}
```

### `QuestionItem`

```rust
pub struct QuestionItem {
    pub question: String,
    /// If non-empty, the user must pick from this list (unless allow_free_text is true).
    pub options: Vec<String>,
    /// Allow an arbitrary text answer even when options are provided.
    pub allow_free_text: bool,
}
```

### `MultiAnswer`

```rust
pub enum MultiAnswer {
    /// User selected one of the provided options.
    Selected { option: String },
    /// User typed a free-text answer.
    Answer { text: String },
    /// User skipped this question.
    Skip,
}
```

---

## Transport

### `IpcStream`

A cross-platform, framed byte stream:

- **Unix** — Unix Domain Socket (`AF_UNIX`)
- **Windows** — Named Pipe (`\\.\pipe\…`)

### Wire helpers

```rust
/// Write a length-prefixed, JSON-encoded IpcRequest to the stream.
pub async fn write_ipc_request(
    stream: &mut IpcStream,
    req: &IpcRequest,
) -> Result<(), ToolError>;

/// Read and decode the next IpcResponse from the stream.
/// Returns None on clean EOF.
pub async fn read_ipc_response(
    stream: &mut IpcStream,
) -> Result<Option<IpcResponse>, ToolError>;
```

Framing format: `[u32 little-endian length][JSON payload]`. Maximum
message size is 64 MB (`MAX_MESSAGE_SIZE` in `ene_tool_proto::ipc`).

### `SandboxConfigData`

A serializable representation of the sandbox policy, sent during `Initialize`. The exact fields are internal and subject to change; tool binaries should treat this as opaque.

---

## Related Pages

- [`ene-tool-host`](ene-tool-host.md) — Host-side lifecycle and registry
- [`ene-tool-common`](ene-tool-common.md) — Tool-side `ToolAction` trait
- [`ene-tool-derive`](ene-tool-derive.md) — Proc-macros for generating `ToolSpec`
