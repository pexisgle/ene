# Tool System (IPC / Host)

Each tool runs as an independent binary process, communicating with the host over IPC (Unix Domain Sockets on Linux, Named Pipes on Windows).

For the human-facing action catalog, see the [Tool catalog](../../guide/tools/overview.md).

## Architecture

```
ToolHostManager (binary discovery, spawning, supervision)
  ├── SupervisedIpcRegistry × N (IPC + restart)
  │   └── IpcToolRegistry (reconnect)
  │       └── ene-tool-proto protocol
  ├── extra_registries × N (MCP, etc.)
  │   └── McpToolRegistry
  └── CompositeToolRegistry
       └── First-wins dedup

ToolRag (separate from registry, owned by EneActor)
  ├── EmbeddingProvider (query + HyDE + rerank)
  ├── MemoryStore (tool_embedding_index)
  └── Weighted multi-field cosine similarity

DbIpcServer × N (per-tool database, Unix only)
  ├── ene-tool-fs  → ene-db-fs.sock   (prefix: fs_)
  ├── ene-tool-utility → ene-db-utility.sock (prefix: utility_)
  └── …
```

## Naming

All tools use namespaced names: `<namespace>.<action>`. Namespace tables live in the [catalog](../../guide/tools/overview.md).

## IPC Protocol (`ene-tool-proto`)

```rust
pub enum IpcRequest {
    Handshake {
        version: u32,
        sandbox: SandboxConfigData,
        tool_config: Option<Value>,
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

pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolSpec> },
    RagProfiles { profiles: Vec<ToolRagProfile> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    Error { message: String },
}
```

Wire format: 4-byte little-endian length prefix + JSON payload. Max message size: 64 MB. Protocol version: `IPC_PROTOCOL_VERSION = 1` (see `crates/ene-tool-proto/src/ipc.rs`).

## ToolHostManager

`ene-tool-host` crate. Orchestrates all tool processes.

| Method | Description |
|--------|-------------|
| `start(config: &EneConfig)` | Creates socket dir, spawns enabled tool binaries, returns `ToolHostManager` |
| `start_full(config: &EneConfig)` | Calls `start()` + connects MCP servers → returns `Arc<dyn ToolRegistry>` (with fallback on failure) |
| `add_registry(registry)` | Registers external registries (e.g., MCP) |
| `into_registry()` | Consumes manager, returns `Arc<dyn ToolRegistry>` |

> **Note:** The `with_store(store)` method shown in earlier drafts of this doc no longer exists on `ToolHostManager`. Tool RAG wiring is performed by `EneActor::reconfigure` via `init_tool_rag(config, embedder, session)` (see `crates/ene-runtime/src/handle.rs`).

### Binary Discovery

`find_tool_binary(name)` searches in order:

1. `builtin_tools_dir()` — debug: same dir as exe, release: `exe_dir/tools/`
2. `user_tools_dir()` — `app_data_dir()/tools/`

### Crash Resilience

| Layer | Behavior |
|-------|----------|
| `SupervisedIpcRegistry` (process) | Process death detection → exponential backoff restart (max 5: 500ms → 8s) |
| `IpcToolRegistry` (connection) | Connection loss → exponential backoff reconnect (base 200ms doubling, cap 10s, 5 retries), re-sends Handshake |
| `ToolHostManager::connect_with_retry` (initial) | Constant 50ms delay, 50 retries (`CONNECT_RETRIES = 50`, `CONNECT_DELAY_MS = 50`) |

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

Implemented by tool binaries. Returns the structured `ToolSpec` type:

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
    fn set_config(&self, _config: &serde_json::Value) {}
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
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
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

Tool RAG is handled separately by the `ToolRag` struct (owned by `EneActor`), not by the registry.

## CompositeToolRegistry

Aggregates multiple `ToolRegistry` instances:

- **First-wins** — duplicate tool names resolve to the first registration
- Dispatches `call_tool`, `set_session_id`, `approve_permission`, `allow_pattern` to the correct sub-registry

## MCP Support

`McpToolRegistry` connects to Model Context Protocol servers:

| Method | Description |
|--------|-------------|
| `connect_stdio(name, cmd, args)` | Launches child process, connects via rmcp |
| `list_tools()` | Merges tool definitions from all servers |
| `call_tool(name, args)` | Dispatch to correct server |

## Custom Tool Registration

Practical steps for humans: [Write a tool](../../guide/tools/write-a-tool.md). Full ABI walkthrough: [SDK Guide](sdk.md).

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": { "foo": "bar" } }
    }
  }
}
```
