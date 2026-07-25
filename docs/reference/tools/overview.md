# Tool System (IPC / Host)

Each tool runs as an independent plugin binary process, communicating with the host over IPC (Unix Domain Sockets on Linux, Named Pipes on Windows). All tool plugins use the unified plugin protocol v3 (`ene-plugin-proto`).

For the human-facing action catalog, see the [Tool catalog](../../guide/tools/overview.md).

## Architecture

```
PluginHostManager (binary discovery, spawning, supervision)
  ├── IpcPluginRegistry × N (IPC + restart)
  │   └── ene-plugin-proto protocol v3
  ├── McpToolRegistry × N (MCP servers)
  ├── ToolRegistry adapter (capabilities.tools → ToolRegistry)
  └── CompositeToolRegistry
       └── Hard-error dedup (DuplicateToolName)

ToolRag (separate from registry, owned by EneActor)
  ├── EmbeddingProvider (query + HyDE + rerank)
  ├── MemoryStore (tool_embedding_index)
  └── Weighted multi-field cosine similarity

DbIpcServer × N (per-tool database, Unix only)
  ├── ene-plugin-fs  → ene-db-fs.sock   (prefix: fs_)
  ├── ene-plugin-utility → ene-db-utility.sock (prefix: utility_)
  └── …
```

## Naming

All tools use namespaced names: `<namespace>.<action>`. Namespace tables live in the [catalog](../../guide/tools/overview.md).

## IPC Protocol (`ene-plugin-proto`)

Plugin IPC uses protocol v3, which extends the legacy tool IPC v2 with streaming LLM messages and a richer handshake. Tool plugins use the tool-related subset of the protocol.

Wire format: 4-byte little-endian length prefix + JSON payload. Max message size: 64 MB. Protocol version: `PLUGIN_IPC_PROTOCOL_VERSION = 3` (see `crates/ene-plugin-proto/src/ipc.rs`).

Key tool-related messages:

```rust
pub enum PluginIpcRequest {
    Handshake {
        version: u32,
        sandbox: SandboxConfigData,
        plugin_config: Option<Value>,
    },
    ListTools,
    ListRagProfiles,
    GetConfigSchema,
    CallTool { name: String, arguments: String, deferred: bool },
    SetCallContext { conversation_id: String, turn_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    RevokePattern { action: String, target_pattern: String },
    Shutdown,
    Ping,
    PollDeferred { task_id: String },
    CancelDeferred { task_id: String },
    // ... streaming LLM messages (CreateChatStream, ChatCompletion, EmbedBatch)
}

pub enum PluginIpcResponse {
    HandshakeAck { version: u32, capabilities: PluginCapabilities },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    RagProfiles { profiles: Vec<ToolRagProfile> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    DeferredAccepted { task_id: String },
    DeferredStatus { task_id: String, status: DeferredStatus },
    Error { message: String },
    Pong,
    // ... streaming responses (StreamChunk, StreamEnd, StreamError)
}
```

## PluginHostManager

`ene-plugin-host` crate. Orchestrates all plugin processes (tools, providers, MCP).

| Method | Description |
|--------|-------------|
| `start(config: &EneConfig)` | Creates socket dir, spawns enabled plugin binaries, connects MCP servers, returns `PluginHostManager` |
| `tool_registries()` | Returns `Arc<dyn ToolRegistry>` combining all tool-capable plugins and MCP servers |
| `shutdown()` | Gracefully shuts down all managed processes |

### Binary Discovery

Plugin binaries are discovered from `builtin_plugins_dir()` and `user_plugins_dir()` (see `ene-config` paths). Binaries must follow the `ene-plugin-{name}` naming convention.

### Crash Resilience

| Layer | Behavior |
|-------|----------|
| Process supervision | Process death detection → exponential backoff restart (max 5: 500ms → 8s) |
| Hang detection | A failed call followed by a failed `Ping` probe marks the plugin unhealthy (alive but unresponsive) and restarts it |
| Connection | Connection loss → exponential backoff reconnect (base 200ms doubling, cap 10s, 5 retries), re-sends Handshake |

### Health Checks and Circuit Breaker

A periodic liveness probe pings every plugin on a fixed interval. A plugin that is dead or fails to answer `Ping` within the probe bound is restarted and its recovery surfaced as a health event.

A per-plugin circuit breaker pauses calls after consecutive failures for a cooldown window, then allows a probe call. A successful call closes the breaker.

Health events are bridged into the runtime diagnostics channel as `DiagnosticEvent::ToolHealth` with a stable English `status` contract: `unhealthy`, `restarting`, `restarted`, `recovered`, `circuit_open`, `circuit_closed`, `disabled`.

## ToolAction Trait

`ene-tool-common` defines the `ToolAction` trait for the action module pattern used by all built-in tool binaries:

```rust
#[async_trait]
pub trait ToolAction: Send + Sync {
    fn definition(&self) -> ToolSpec;
    fn tool_name(&self) -> &'static str;
    async fn execute(&self, arguments: &str) -> Result<String, ToolError>;
}
```

Each tool binary has a provider struct that owns shared state and dispatches to individual `ToolAction` implementations by `tool_name()`.

## ToolProvider Trait

Implemented by tool binaries (defined in `ene-plugin-proto`). Returns the structured `ToolSpec` type:

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn list_specs(&self) -> Vec<ToolSpec>;
    fn list_rag_profiles(&self) -> Vec<ToolRagProfile> { vec![] }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    fn set_session_id(&self, session_id: &str);
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}
    fn approve_permission(&self, _request_id: &str) {}
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}
    fn set_config(&self, _config: &serde_json::Value) {}
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolPluginAdapter

`ene-plugin` provides `ToolPluginAdapter<T: ToolProvider>` which wraps any `ToolProvider` into the unified `Plugin` trait, allowing tool binaries to be served via `run_plugin_server`:

```rust
use ene_plugin::{ToolPluginAdapter, run_plugin_server};

let provider = ActionSetProvider::new(vec![/* actions */]);
run_plugin_server(Box::new(ToolPluginAdapter(provider))).await?;
```

## ToolRegistry Trait

Host-side interface for tool access:

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolSpec>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    async fn set_session_id(&self, _session_id: &str) {}
    async fn approve_permission(&self, _request_id: &str) {}
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    async fn revoke_pattern(&self, _action: &str, _target_pattern: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

Tool RAG is handled separately by the `ToolRag` struct (owned by `EneActor`), not by the registry.

## CompositeToolRegistry

Aggregates multiple `ToolRegistry` instances:

- **Duplicate tool names are a hard error** — registering a duplicate returns `DuplicateToolName`. This aligns with API v1 (#135): all tools must have unique public names.
- Dispatches `call_tool`, `set_session_id`, `approve_permission`, `allow_pattern`, `revoke_pattern` to the correct sub-registry

## MCP Support

`McpToolRegistry` connects to Model Context Protocol servers:

| Method | Description |
|--------|-------------|
| `connect_stdio(name, command, args)` | Launches child process, connects via rmcp |
| `list_tools()` | Merges tool definitions from all servers |
| `call_tool(name, args)` | Dispatch to correct server |

MCP servers are configured under `plugins.mcp_servers` (see [Settings](../configuration/settings.md#plugins--plugin-system)).

## Custom Tool Registration

Practical steps for humans: [Write a tool](../../guide/tools/write-a-tool.md). Full ABI walkthrough: [SDK Guide](sdk.md).

```json
{
  "plugins": {
    "list": {
      "my-tool": { "enable": true }
    }
  }
}
```
