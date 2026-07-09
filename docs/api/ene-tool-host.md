# `ene-tool-host` — API Reference

> **Crate:** `ene-tool-host`
> **Role:** Tool process lifecycle, IPC client management, MCP server connections, and the Tool RAG selection pipeline.

---

## Overview

`ene-tool-host` is the bridge between `ene-core` and the standalone tool binaries. It is responsible for:

1. **Spawning and supervising** tool child processes, with automatic reconnect and restart on crash.
2. **Negotiating** the IPC handshake and maintaining persistent Unix-socket connections.
3. **Routing** `call_tool` requests to the right registry and returning results to the core actor.
4. **Connecting to MCP servers** (stdio transport) and exposing their tools through the same `ToolRegistry` interface.
5. **Running the Tool RAG pipeline** to select a relevant subset of tools for each turn when the full tool list would exceed the LLM's context budget.

See also: [`ene-tool-proto`](./ene-tool-proto.md) for the wire types (`ToolSpec`, `IpcRequest`/`IpcResponse`, `ToolError`), and [`ene-tool-common`](./ene-tool-common.md) for the tool-side API.

---

## `ToolRegistry` trait

The central abstraction. `IpcToolRegistry`, `McpToolRegistry`, `CompositeToolRegistry`, and `ToolHostManager` all implement it, enabling composition.

```rust
#[async_trait]
pub trait ToolRegistry: Send + Sync {
    /// Returns the list of all available tools.
    fn list_tools(&self) -> Vec<ToolSpec>;
    /// Executes a tool by name with the given JSON arguments from the LLM.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>;

    /// Sets the current session ID (used for undo tracking, session-scoped state).
    async fn set_session_id(&self, _session_id: &str) {}
    /// Approves a pending destructive-operation permission request by ID.
    async fn approve_permission(&self, _request_id: &str) {}
    /// Adds a session-wide permission allow pattern (action + target glob).
    async fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}
    /// Returns the JSON Schema for the tool's config section in settings.json.
    async fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `list_tools` | `fn list_tools(&self) -> Vec<ToolSpec>` | Synchronous — returns the cached tool list. |
| `call_tool` | `async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError>` | Dispatches JSON-encoded arguments to the named tool. Returns the tool's text output, or a `ToolHostError`. |
| `set_session_id` | `async fn set_session_id(&self, session_id: &str)` | Default no-op. Propagates the current session ID to connected tool processes. |
| `approve_permission` | `async fn approve_permission(&self, request_id: &str)` | Default no-op. Grants a pending permission request. |
| `allow_pattern` | `async fn allow_pattern(&self, action: &str, target_pattern: &str)` | Default no-op. Adds a glob pattern to the sandbox allow-list for an action class. |
| `config_schema` | `async fn config_schema(&self) -> Option<serde_json::Value>` | Default `None`. Returns the JSON Schema for this registry's configuration. |

> **Note:** `call_tool` returns `Result<String, ToolHostError>` — this crate's own error type, *not* `ene_tool_proto::ToolError`. `ToolHostError` wraps the protocol-level `ToolError` (see [Errors](#errors-toolhosterror--eneToolhosterror) below).

---

## `ToolHostManager`

The top-level manager that spawns all configured tool processes and connects to configured MCP servers, then assembles them into one composite registry.

```rust
pub struct ToolHostManager { /* private */ }
```

### Constructors

| Method | Signature | Description |
|--------|-----------|-------------|
| `start` | `pub async fn start(config: &EneConfig, db_tokens: HashMap<String, String>) -> Result<Self, ToolHostError>` | Reads `config.tool`, spawns every `enabled` tool process (wrapped in a supervised, auto-reconnecting registry), and connects configured MCP servers. `db_tokens` maps tool name → per-tool database auth token, consumed and forwarded into each tool's sandbox config as it is spawned. Returns an unstarted manager you can extend with `add_registry`. |
| `start_full` | `pub async fn start_full(config: &EneConfig, db_tokens: HashMap<String, String>) -> Result<Arc<dyn ToolRegistry>, ToolHostError>` | Convenience wrapper: calls `start` then `into_registry`. Use this in most applications. |

### Instance methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `add_registry` | `pub fn add_registry(&mut self, registry: Arc<dyn ToolRegistry>)` | Appends an additional registry (e.g. a custom in-process registry) before conversion. Delegates to `CompositeToolRegistry::add_registry`. |
| `into_registry` | `pub fn into_registry(self) -> Arc<dyn ToolRegistry>` | Consumes the manager and returns it as `Arc<dyn ToolRegistry>` (it implements the trait itself, delegating to its internal composite). |

### Example

```rust,no_run
use ene_tool_host::ToolHostManager;
use std::collections::HashMap;

# async fn run(config: &ene_config::EneConfig) -> Result<(), Box<dyn std::error::Error>> {
let db_tokens: HashMap<String, String> = HashMap::new();
let registry = ToolHostManager::start_full(config, db_tokens).await?;

let tools = registry.list_tools();
println!("Loaded {} tools", tools.len());

let result = registry.call_tool("fs_read_file", r#"{"path":"/tmp/foo.txt"}"#).await?;
println!("{result}");
# Ok(())
# }
```

Internally, each spawned tool process is wrapped in a private `SupervisedIpcRegistry` (see [below](#internal-process-supervision)) rather than a bare `IpcToolRegistry` — this is what gives `ToolHostManager` its crash-restart behavior. `SupervisedIpcRegistry` is **not** part of the public API.

---

## `IpcToolRegistry`

Manages a persistent IPC connection to a single tool binary subprocess over a Unix domain socket, with automatic reconnection.

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

### Constructor & methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub async fn new(socket_path: PathBuf, sandbox: SandboxConfigData, tool_config: Option<serde_json::Value>, timeout_ms: u64) -> Result<Self, ToolHostError>` | Connects to the socket, performs the handshake/initialize/list-tools sequence, and caches the resulting `ToolSpec`s. `timeout_ms` bounds every subsequent request (from `ToolConfig::timeout_ms`) so a hung tool call cannot block the host indefinitely. |
| `refresh_tools` | `pub async fn refresh_tools(&self) -> Result<(), ToolHostError>` | Re-sends `ListTools` and updates the internal cache. Useful after a hot-reload. |
| `socket_path` | `pub fn socket_path(&self) -> &PathBuf` | Returns the IPC socket path this registry is connected to. |
| `get_config_schema` | `pub async fn get_config_schema(&self) -> Option<serde_json::Value>` | Fetches the tool binary's config schema via `GetConfigSchema`. |

There is **no public `connect` method** — reconnection is handled transparently and privately (`connect_with_retry`, `ensure_connected`, `send_with_reconnect`) inside every call made through the `ToolRegistry` trait impl.

### Automatic reconnection

If the connection drops mid-session, `IpcToolRegistry` reconnects with exponential backoff:

| Attempt | Delay before next try |
|---|---|
| 1 | 200 ms |
| 2 | 400 ms |
| 3 | 800 ms |
| 4 | 1.6 s |
| 5 | gives up — returns `ToolHostError` |

---

## Internal Process Supervision

> `SupervisedIpcRegistry` is a **private** implementation detail of `tool_host_manager.rs` (no `pub` modifier) and is intentionally **not** part of the documented public API — it is only reachable as an opaque `Arc<dyn ToolRegistry>` returned from `ToolHostManager::start`.

It wraps an `IpcToolRegistry` with process-level supervision: if the child process crashes, it is restarted with exponential backoff (`BASE_DELAY_MS = 500`, doubling per attempt, capped at `MAX_DELAY_MS = 30_000`), and the wrapped `IpcToolRegistry` is reconnected afterward using `ToolHostManager`'s internal constant retry policy (`CONNECT_RETRIES = 50` attempts, `CONNECT_DELAY_MS = 50` ms apart).

---

## `CompositeToolRegistry`

Aggregates multiple `ToolRegistry` implementations behind one, with **O(1)** dispatch by tool name.

```rust
pub struct CompositeToolRegistry {
    state: RwLock<CompositeState>,
}

struct CompositeState {
    registries: Vec<Arc<dyn ToolRegistry>>,
    /// Maps tool_name -> index into `registries`, built in `new`/`add_registry`.
    tool_index: HashMap<String, usize>,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub fn new(registries: Vec<Arc<dyn ToolRegistry>>) -> Self` | Builds the composite and its `tool_index` from the given registries in order. |
| `add_registry` | `pub fn add_registry(&self, registry: Arc<dyn ToolRegistry>)` | Appends a registry and indexes its tools. Takes `&self` (uses an internal `RwLock`), so it can be called after the composite is shared. |

`call_tool` looks up `tool_index.get(name)` to find the owning registry in O(1) rather than scanning every sub-registry's `list_tools()`, then dispatches directly to it — returning `ToolHostError::Protocol(ToolError::NotFound { .. })` if the name isn't indexed. On duplicate tool names across registries, the **first** registration wins (`HashMap::entry(..).or_insert(idx)`).

---

## MCP Integration

### `McpServerConnection`

```rust
/// Represents a connection to an MCP server.
pub struct McpServerConnection {
    pub name: String,
    pub client: Arc<rmcp::Peer<rmcp::RoleClient>>,
    pub tools: Vec<ToolSpec>,
}
```

### `McpToolRegistry`

An adapter that wraps one or more MCP (Model Context Protocol) client connections as a single `ToolRegistry`, letting Ene call tools exposed by any MCP-compatible server.

```rust
#[derive(Default)]
pub struct McpToolRegistry {
    servers: Arc<RwLock<Vec<McpServerConnection>>>,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub fn new() -> Self` | Creates an empty registry (equivalent to `Self::default()`). |
| `connect_stdio` | `pub async fn connect_stdio(&self, name: &str, command: &str, args: &[&str]) -> Result<(), ToolHostError>` | Spawns `command` as a child process and connects via MCP's stdio transport, then lists and caches its tools under `name`. The underlying `rmcp` client's string-based errors are wrapped as `ToolHostError::ExecutionFailed { message }` (see the [API refactor plan](../architecture/api-refactor-plan.md), item 3 — this used to return a bare `Result<(), String>`). |

Configured via `ToolConfig::mcp_servers`; `McpTransport::Http` is accepted by config/schema but not yet implemented in `ToolHostManager::start` (logs a warning and is skipped).

```rust
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
}

#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}
```

---

## Tool RAG Pipeline

When the number of available tools exceeds the LLM's context budget, `ToolRag` runs a retrieval-augmented selection step: embed → optional HyDE expansion → weighted multi-field similarity → optional cross-encoder rerank → top-N.

### `ToolRag`

```rust
pub struct ToolRag {
    embedder: Arc<dyn EmbeddingProvider>,
    store: Option<Arc<MemoryStore>>,
    opts: ToolRagOptions,
    specs: RwLock<HashMap<ToolName, ToolSpec>>,
    last_specs_hash: AtomicU64,
    cached_field_rows: RwLock<Vec<CachedFieldRow>>,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `new` | `pub fn new(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, opts: ToolRagOptions) -> Self` | Direct constructor when you already have a resolved `ToolRagOptions`. |
| `from_config` | `pub fn from_config(embedder: Arc<dyn EmbeddingProvider>, store: Option<Arc<MemoryStore>>, config: ToolRagConfig) -> Result<Self, ToolHostError>` | Builds `ToolRagOptions` from the settings-facing `ToolRagConfig` (converting `Vec<String>` → `Vec<ToolName>` for `forced`) and constructs the pipeline. |
| `ensure_index` | `pub async fn ensure_index(&self, specs: &[ToolSpec]) -> Result<(), EmbeddingError>` | Computes a BLAKE3 hash over the spec set; if unchanged since the last call, this is a fast no-op. Otherwise (re-)embeds and stores per-field vectors for changed tools. |
| `select` | `pub async fn select(&self, query: &str) -> Vec<ToolSpec>` | Embeds `query` internally, then delegates to `select_with_embedding`. |
| `select_with_embedding` | `pub async fn select_with_embedding(&self, query: &str, query_embedding: &[f32]) -> Vec<ToolSpec>` | Runs weighted per-field similarity scoring (using `query_embedding`) plus optional HyDE blending and reranking, and returns the top `opts.final_n` tools above `opts.min_similarity`, always including any `opts.forced` tools. |
| `start_background_indexer` | `pub fn start_background_indexer(self: &Arc<Self>, specs: Vec<ToolSpec>)` | Spawns a background task that calls `ensure_index` to warm the cache; returns immediately. |
| `stats` | `pub async fn stats(&self) -> ToolRagStats` | Snapshot of the last `select`/`select_with_embedding` call: hit count, index size, and top similarity. |
| `opts` | `pub fn opts(&self) -> &ToolRagOptions` | Returns the resolved options. |
| `has_store` | `pub fn has_store(&self) -> bool` | Whether a backing `MemoryStore` is attached (RAG requires one to persist embeddings across restarts). |

### `ToolRagOptions`

```rust
#[derive(Debug, Clone)]
pub struct ToolRagOptions {
    pub enabled: bool,
    /// Number of top candidates retrieved from vector search.
    pub top_k: usize,
    /// Number of final tools returned after reranking.
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    /// Number of candidates considered during reranking.
    pub rerank_candidates: usize,
    /// Minimum similarity score for a tool to be included.
    pub min_similarity: f32,
    /// Whether to warm the index in the background on startup.
    pub background_index_on_startup: bool,
    /// Tools always included regardless of RAG scoring.
    pub forced: Vec<ToolName>,
    /// Per-field embedding weights used for scoring.
    pub weights: FieldWeights,
}
```

### `FieldWeights`

Controls how strongly each embedding field contributes to a tool's relevance score. Negative weights (e.g. on `negative`) act as a soft penalty rather than a hard exclusion.

```rust
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub summary: f32,
    pub description: f32,
    pub example: f32,
    /// Weight for the negative/unwanted embedding (penalizes matches).
    pub negative: f32,
    /// Weight for the HyDE (hypothetical document embedding).
    pub hyde: f32,
    /// Fraction of the final score contributed by the HyDE similarity,
    /// with the remainder from the direct per-field cosine similarity.
    /// `0.0` disables HyDE blending; `1.0` uses only the HyDE similarity.
    pub hyde_blend: f32,
}
```

`FieldWeightsConfig` (in `config.rs`) is the serializable/schema counterpart of `FieldWeights` with the same fields — `impl From<FieldWeightsConfig> for FieldWeights` converts between them. Keep both in sync if you add a field.

### `ToolRagStats`

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolRagStats {
    /// Tools returned in the most recent `select` call.
    pub hits: usize,
    /// Total distinct tools in the index.
    pub total: usize,
    /// Cosine similarity of the best match in the most recent call.
    pub top_similarity: f32,
}
```

### `compute_tool_version_hash`

```rust
pub fn compute_tool_version_hash(tool: &ToolSpec) -> String
```

Computes a stable BLAKE3 hash used to invalidate cached tool embeddings whenever a tool's spec changes meaningfully. The hash covers `tool.name`, `tool.version`, **`tool.display_name`**, **`tool.summary`**, `tool.description`, `tool.parameters`, and all four `keywords` tiers (`primary`, `secondary`, `domain`, `negative`). When this hash changes for a tool (tracked via `list_tool_embedding_hashes` in `ene-memory`), `ToolRag::ensure_index` re-embeds it.

---

## Configuration Types

These mirror the `[tools]` section of `assets/settings.json`, loaded via `ene-config`'s `define_config!` macro.

### `ToolConfig`

```rust
pub struct ToolConfig {
    pub enabled: bool = true,
    /// Maximum number of sequential tool-call rounds per turn.
    pub max_rounds: usize = 10,
    pub timeout_ms: u64 = 60_000,
    pub list: HashMap<String, ToolEntry>,
    pub mcp_servers: Vec<McpServerConfig>,
    pub rag: ToolRagConfig,
}
```

### `ToolEntry`

```rust
pub struct ToolEntry {
    pub enable: bool,
    /// Tool-specific configuration (flattened into the parent JSON object).
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl ToolEntry {
    /// Type-safe deserialization of `config` into a tool-specific settings struct.
    pub fn deserialize_config<T: DeserializeOwned>(&self) -> Result<T, serde_json::Error>;
}
```

### `ToolRagConfig`

```rust
pub struct ToolRagConfig {
    pub enabled: bool,
    pub top_k: usize,
    pub final_n: usize,
    pub use_hyde: bool,
    pub use_rerank: bool,
    pub rerank_candidates: usize,
    pub min_similarity: f32,
    pub background_index_on_startup: bool,
    pub forced: Vec<String>,
    pub weights: FieldWeightsConfig,
}
```

---

## Errors: `ToolHostError` / `EneToolHostError`

```rust
#[derive(Debug, Error)]
pub enum EneToolHostError {
    /// Error originating from the underlying tool protocol (IPC).
    #[error(transparent)]
    Protocol(#[from] ene_tool_proto::ToolError),
    /// Configuration error (e.g. invalid RAG config).
    #[error("Configuration error: {0}")]
    Config(String),
    /// I/O error during tool spawning or socket management.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Execution failed (e.g. tool binary not found, MCP client failed to start).
    #[error("Execution failed: {message}")]
    ExecutionFailed { message: String },
}

impl EneToolHostError {
    /// Creates a `Protocol(ToolError::IpcClient { .. })` error with the given message.
    pub fn ipc(message: impl Into<String>) -> Self;
}

/// Alias used throughout the crate's public API.
pub type ToolHostError = EneToolHostError;
```

---

## See Also

- [`ene-tool-proto`](./ene-tool-proto.md) — IPC wire types (`ToolSpec`, `IpcRequest`/`IpcResponse`, `ToolError`)
- [`ene-tool-common`](./ene-tool-common.md) — Tool-side `ToolAction` trait and helpers for tool binaries
- [`ene-tool-derive`](./ene-tool-derive.md) — Proc-macros for tool authors (`#[derive(ToolSpec)]`)
- [`ene-memory`](./ene-memory.md) — Backs `ToolRag`'s persistent embedding index (`tool_embedding_index` table)
- [`ene-core`](./ene-core.md) — Owns the `Arc<dyn ToolRegistry>` returned by `start_full` and drives tool calls from the actor loop
- [Tool System Overview](../tools/overview.md)
