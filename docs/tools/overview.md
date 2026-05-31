# Tool System Overview

Each tool type runs as an independent binary process, communicating with core over IPC (Unix Domain Sockets on Linux, Named Pipes on Windows).

## Architecture

```
ToolHostManager (binary discovery, spawning, supervision)
  ├── SupervisedIpcRegistry × N (IPC + restart)
  │   └── IpcToolRegistry (reconnect)
  │       └── ene-tool-proto protocol
  ├── extra_registries × N (MCP, etc.)
  │   └── McpToolRegistry
  └── MemoryStore (Tool RAG)
```

## IPC Protocol (`ene-tool-proto`)

```rust
pub enum IpcRequest {
    Handshake { version: u32 },
    Initialize { sandbox: SandboxConfigData, tool_config: Option<Value> },
    ListTools,
    GetConfigSchema,
    CallTool { name: String, arguments: String },
    SetSessionId { session_id: String },
    ApprovePermission { request_id: String },
    AllowPattern { action: String, target_pattern: String },
    Ping,
    Shutdown,
}

pub enum IpcResponse {
    HandshakeAck { version: u32 },
    Ack,
    Tools { tools: Vec<ToolDefinition> },
    ConfigSchema { schema: Option<Value> },
    CallResult { result: Result<String, ToolError> },
    Pong,
    Error { message: String },
}
```

Wire format: 4-byte little-endian length prefix + JSON payload.

## ToolHostManager

`ene-tool-host` crate. Orchestrates all tool processes.

| Method | Description |
|--------|-------------|
| `start(config)` | Creates socket dir, spawns enabled tool binaries, returns `ToolHostManager` |
| `start_full(config)` | Calls `start()` + connects MCP servers → returns `Arc<dyn ToolRegistry>` (with fallback on failure) |
| `add_registry(registry)` | Registers external registries (e.g., MCP) |
| `with_store(store)` | Attaches MemoryStore for Tool RAG |
| `into_registry()` | Consumes manager, returns `Arc<dyn ToolRegistry>` |

### Binary Discovery

`find_tool_binary(name)` searches in order:
1. `builtin_tools_dir()` — debug: same dir as exe, release: `exe_dir/tools/`
2. `user_tools_dir()` — `app_data_dir()/tools/`

### Crash Resilience

| Layer | Behavior |
|-------|----------|
| ToolHostManager | Process death detection → exponential backoff restart (max 5, 500ms → 30s) |
| IpcToolRegistry | Connection loss → exponential backoff reconnect (max 5), re-sends Initialize |

## ToolRegistry Trait

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolDefinition>;
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;
    async fn set_session_id(&self, session_id: &str) {}
    async fn approve_permission(&self, request_id: &str) {}
    async fn allow_pattern(&self, action: &str, target_pattern: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
    async fn ensure_index_built(&self, embedder: &dyn EmbeddingProvider, store: Option<&MemoryStore>) -> Result<(), ToolError> { Ok(()) }
    async fn select_tools(&self, embedder: &dyn EmbeddingProvider, query: &str, limit: usize) -> Vec<ToolDefinition> { self.list_tools() }
}
```

## CompositeToolRegistry

Aggregates multiple `ToolRegistry` instances:

- **First-wins** — duplicate tool names resolve to the first registration
- **Tool RAG** — `ensure_tool_embeddings()` computes version hashes, re-embeds only changed tools via `store.upsert_tool_embedding()`
- **`select_tools()`** — cosine-similarity filtering using stored tool embeddings, with `tool_rag_always_include` tools always present

## MCP Support

`McpToolRegistry` connects to Model Context Protocol servers:

| Method | Description |
|--------|-------------|
| `connect_stdio(name, cmd, args)` | Launches child process, connects via rmcp |
| `list_tools()` | Merges tool definitions from all servers |
| `call_tool(name, args)` | Dispatch to correct server |

## Custom Tool Registration

1. Implement `ToolProvider` trait from `ene-tool-proto`
2. Call `run_tool_server()` in your binary's `main()`
3. Place binary in `~/.local/share/dev.pexisgle.ene/tools/`
4. Add entry to `settings.json` under `tools.tools`

```json
{
  "tools": {
    "tools": {
      "my-tool": { "enable": true, "config": { "foo": "bar" } }
    }
  }
}
```

See [SDK Guide](sdk.md) for a complete walkthrough.
