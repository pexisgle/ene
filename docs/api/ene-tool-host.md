# `ene-tool-host`

> Tool process lifecycle, IPC client management, and the Tool RAG pipeline.

`ene-tool-host` is the bridge between `ene-core` and the standalone tool binaries. It is responsible for:

1. **Spawning and supervising** tool child processes.
2. **Negotiating** the IPC handshake and maintaining persistent connections.
3. **Routing** `CallTool` requests and streaming results back to the core actor.
4. **Running the Tool RAG pipeline** to select a relevant subset of tools for each turn.

See also: [`ene-tool-proto`](ene-tool-proto.md) for the wire types, [`ene-tool-common`](ene-tool-common.md) for the tool-side API.

---

## `ToolRegistry` trait

The central abstraction. Both the host manager and individual registries implement this trait, enabling composition.

```rust
#[async_trait::async_trait]
pub trait ToolRegistry: Send + Sync {
    fn list_tools(&self) -> Vec<ToolSpec>;
    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, ToolError>;

    // Optional hooks — default implementations are no-ops.
    async fn set_session_id(&self, _session_id: &str) {}
    async fn approve_permission(&self, _request_id: &str) {}
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

### Methods

| Method | Description |
|---|---|
| `list_tools()` | Returns the full list of [`ToolSpec`](ene-tool-proto.md#toolspec)s exposed by this registry. |
| `call_tool(name, arguments)` | Dispatches a JSON-encoded argument string to the named tool. Returns the tool's text output or a [`ToolError`](ene-tool-proto.md#toolerror). |
| `set_session_id(session_id)` | Propagates the current session ID to all connected tool processes so they can scope their state. |
| `approve_permission(request_id)` | Grants a pending permission request (used when a tool emits `ToolError::PermissionRequired`). |
| `allow_pattern(action, target_pattern)` | Adds a glob/regex pattern to the sandbox allow-list for a given action class. |
| `config_schema()` | Returns the JSON Schema for this registry's configuration, if any. |

---

## `ToolHostManager`

The top-level manager that assembles a composite registry from all configured tool processes and MCP servers.

```rust
pub struct ToolHostManager { /* private */ }
```

### Constructors

| Method | Description |
|---|---|
| `ToolHostManager::start(config: &EneConfig) -> Result<Self, ToolError>` | Reads `config.tool` and spawns only the tool processes that are `enabled`. Returns an unstarted manager you can extend with `add_registry`. |
| `ToolHostManager::start_full(config: &EneConfig) -> Result<Arc<dyn ToolRegistry>, ToolError>` | Convenience wrapper: calls `start` then `into_registry`. Use this in most applications. |

### Instance methods

| Method | Description |
|---|---|
| `add_registry(&mut self, registry: Arc<dyn ToolRegistry>)` | Appends an additional registry (e.g. a custom in-process registry or an MCP server) before the manager is converted. |
| `into_registry(self) -> Arc<dyn ToolRegistry>` | Consumes the manager and returns a `CompositeToolRegistry` wrapping all registered registries. |

### Example

```rust
use ene_tool_host::ToolHostManager;

let registry = ToolHostManager::start_full(&config).await?;
let tools = registry.list_tools();
println!("Loaded {} tools", tools.len());

let result = registry.call_tool("fs.read_file", r#"{"path":"/tmp/foo.txt"}"#)?;
println!("{result}");
```

---

## `IpcToolRegistry`

Manages a persistent IPC connection to a single tool binary subprocess.

```rust
pub struct IpcToolRegistry { /* private */ }
```

### Connection lifecycle

```
Spawn process
     │
     ▼
Handshake  { version: IPC_PROTOCOL_VERSION }
     │
     ▼
Initialize { sandbox, tool_config }
     │
     ▼
ListTools  → caches Vec<ToolSpec>
     │
     ▼
Ready for CallTool / SetSessionId / …
```

If the connection drops, `IpcToolRegistry` performs **automatic reconnection** with exponential backoff (`RECONNECT_BASE_DELAY_MS = 200`, `RECONNECT_MAX_DELAY_MS = 10_000`, `RECONNECT_MAX_RETRIES = 5`):

| Attempt | Delay before next try |
|---|---|
| 1 | 200 ms |
| 2 | 400 ms |
| 3 | 800 ms |
| 4 | 1.6 s |
| 5 | (gives up — returns `ToolError::IpcClient`) |

### Key methods

| Method | Description |
|---|---|
| `refresh_tools(&self) -> Result<(), ToolError>` | Re-sends `ListTools` and updates the internal `ToolSpec` cache. Useful after a hot-reload. |
| `socket_path(&self) -> &PathBuf` | Returns the IPC socket path this registry is connected to. |
| `get_config_schema(&self) -> Option<serde_json::Value>` | Fetches the tool binary's config schema via `GetConfigSchema`. |

---

## `SupervisedIpcRegistry`

Wraps an `IpcToolRegistry` with **process-level supervision**. If the child process crashes, `SupervisedIpcRegistry` restarts it.

The delay between restarts is exponential (`BASE_DELAY_MS = 500`, doubling `2^attempt`, capped at `MAX_DELAY_MS = 30_000`):

| Restart # | Delay before next try |
|---|---|
| 1 | 500 ms |
| 2 | 1 s |
| 3 | 2 s |
| 4 | 4 s |
| 5 | 8 s |
| > 5 | Gives up, returns `ToolError::ExecutionFailed` |

The post-restart reconnection (handled inside `IpcToolRegistry`) uses a **constant 50 ms delay with up to 50 retries** (`CONNECT_DELAY_MS = 50`, `CONNECT_RETRIES = 50` in `tool_host_manager.rs`).

This is used automatically by `ToolHostManager` for all spawned tool processes.

---

## `CompositeToolRegistry`

Combines multiple `ToolRegistry` implementations into one.

```rust
pub struct CompositeToolRegistry {
    registries: Vec<Arc<dyn ToolRegistry>>,
}
```

- `list_tools()` — concatenates the tool lists from all inner registries.
- `call_tool(name, arguments)` — dispatches to the first registry whose `list_tools()` contains a tool with the given name.
- All other methods are forwarded to all inner registries.

---

## `McpToolRegistry`

An adapter that wraps an MCP (Model Context Protocol) client as a `ToolRegistry`. Allows Ene to call tools exposed by any MCP-compatible server.

```rust
pub struct McpToolRegistry { /* private */ }
```

Configured via `ToolConfig::mcp_servers`. The MCP transport (stdio, SSE, etc.) is handled internally.

---

## Tool RAG Pipeline

When the number of available tools exceeds the LLM's context budget, `ene-tool-host` runs a retrieval-augmented selection step to pick the most relevant tools for the current query.

### `ToolRag`

```rust
pub struct ToolRag { /* private */ }
```

| Method | Description |
|---|---|
| `ensure_index(tools: &[ToolSpec]) -> Result<(), ...>` | Computes a BLAKE3 content hash for each tool and (re-)indexes any that have changed. |
| `select(query: &str) -> Vec<ToolSpec>` | Embeds `query` and returns the top-K most similar tools, filtered by `similarity_threshold`. |

### `ToolRagOptions`

```rust
pub struct ToolRagOptions {
    /// Minimum cosine similarity for a tool to be included.
    pub similarity_threshold: f32,
    /// Maximum number of tools to return.
    pub top_k: usize,
    /// Use HyDE (Hypothetical Document Embeddings) to improve retrieval.
    pub use_hyde: bool,
    /// Re-rank results with a cross-encoder after initial retrieval.
    pub use_rerank: bool,
}
```

### `ToolRagStats`

Returned alongside selected tools for observability:

```rust
pub struct ToolRagStats {
    /// Number of tools returned.
    pub hits: usize,
    /// Total tools in the index.
    pub total: usize,
    /// Cosine similarity of the best match.
    pub top_similarity: f32,
}
```

### Version hashing

```rust
pub fn compute_tool_version_hash(tool: &ToolSpec) -> String
```

Computes a BLAKE3 hash over the tool's `name`, `version`, `description`, `parameters`, and `keywords`. The hash is stored in the vector index; when it changes, the tool's embedding is recomputed.

---

## Configuration Types

These types mirror the `[tool]` section of `assets/settings.json` and are loaded via `ene-config`.

### `ToolConfig`

```rust
pub struct ToolConfig {
    /// Whether the tool system is enabled at all.
    pub enabled: bool,
    /// Maximum LLM ↔ tool round-trips per turn.
    pub max_rounds: u32,
    /// Per-call timeout in milliseconds.
    pub timeout_ms: u64,
    /// Per-tool enable flags and config overrides.
    pub list: HashMap<String, ToolEntry>,
    /// MCP server definitions.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Tool RAG settings.
    pub rag: ToolRagConfig,
}
```

### `ToolEntry`

```rust
pub struct ToolEntry {
    /// Whether this specific tool is enabled.
    pub enable: bool,
    /// Arbitrary JSON passed to the tool binary as its config.
    pub config: serde_json::Value,
}
```

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub similarity_threshold: f32,
    pub top_k: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
}
```

### `FieldWeightsConfig` / `FieldWeights`

Control per-field weighting when computing the composite embedding for a `ToolSpec`.

```rust
pub struct FieldWeightsConfig {
    pub summary: f32,
    pub description: f32,
    pub keywords_primary: f32,
    pub keywords_secondary: f32,
    pub keywords_domain: f32,
    pub examples: f32,
}
```

---

## Related Pages

- [`ene-tool-proto`](ene-tool-proto.md) — IPC wire types (`ToolSpec`, `IpcRequest`, `ToolError`)
- [`ene-tool-common`](ene-tool-common.md) — Tool-side `ToolAction` trait
- [`ene-tool-derive`](ene-tool-derive.md) — Proc-macros for tool authors
- [Tool System Overview](../tools/overview.md)
