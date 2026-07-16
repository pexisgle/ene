# SDK: Building Custom Tools

`ene-tool-proto` is the lightweight SDK for building custom tool binaries that integrate with ene.

## Quick Start

### 1. Create a Project

```toml
# Cargo.toml
[package]
name = "my-cool-tool"
version = "0.1.0"
edition = "2024"

[dependencies]
ene-tool-common = { git = "https://github.com/pexisgle/ene" }
ene-tool-proto = { git = "https://github.com/pexisgle/ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/ene" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
async-trait = "0.1"
```

### 2. Define Actions with `#[derive(ToolAction)]`

Each tool is a struct with `#[derive(ToolAction)]`. The derive macro generates the `ToolSpec`, JSON Schema, and `ToolAction` impl. You write the business logic in `async fn run(&self)`.

```rust
use ene_tool_common::prelude::*;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloAction {
    /// Name to greet.
    name: String,
}

impl HelloAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("Hello, {}!", self.name))
    }
}
```

### 3. Implement ToolProvider

For a single stateless action, use [`ActionSetProvider`](#toolprovider-adapters) with a single-element vec instead of hand-writing a `ToolProvider` — this is the recommended default:

```rust
use ene_tool_common::ActionSetProvider;

let provider = ActionSetProvider::new(vec![Box::new(HelloAction::default())]);
```

If you need custom `set_call_context`/`set_sandbox` behavior, or want full control, implement `ToolProvider` by hand instead:

```rust
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec};

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![HelloAction::default().definition()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            HelloAction::TOOL_NAME => {
                let action = HelloAction::default();
                action.execute(arguments).await
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

### 4. Start the Server

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 5. Build and Deploy

```bash
cargo build --release
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 6. Enable in Settings

```json
{
  "tools": {
    "tools": {
      "my-cool-tool": { "enable": true }
    }
  }
}
```

## Stateful Actions

For actions that need injected dependencies (sandbox, database, HTTP client), use `#[tool(skip)]`:

```rust
use ene_tool_common::prelude::*;
use std::sync::Arc;

fn default_client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "myapi",
    name = "fetch",
    summary = "Fetch data from the API.",
    category = "WebFetch"
)]
pub struct FetchAction {
    /// The endpoint to call.
    endpoint: String,

    #[tool(skip)]
    #[serde(skip, default = "default_client")]
    client: reqwest::Client,
}

impl FetchAction {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            endpoint: String::new(),
            client,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let resp = self.client.get(&self.endpoint).send().await
            .map_err(|e| ToolError::ExecutionFailed { message: e.to_string() })?;
        Ok(resp.text().await.unwrap_or_default())
    }
}
```

The provider constructs the action with real dependencies, then `execute()` copies them in:

```rust
async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
    match name {
        FetchAction::TOOL_NAME => {
            let action = FetchAction::new(self.client.clone());
            action.execute(arguments).await
        }
        _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
    }
}
```

## Multiple Tools

Expose multiple tools from a single provider. With [`ActionSetProvider`](#toolprovider-adapters), you don't need to hand-write the dispatch `match`:

```rust
use ene_tool_common::ActionSetProvider;

let provider = ActionSetProvider::new(vec![
    Box::new(AddAction::default()),
    Box::new(SubtractAction::default()),
]);
```

The equivalent hand-written form (useful if you need dispatch logic beyond simple name-matching):

```rust
#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddAction { a: f64, b: f64 }

impl AddAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} + {} = {}", self.a, self.b, self.a + self.b))
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(namespace = "calculator", name = "subtract", summary = "Subtract two numbers.", category = "Utility")]
pub struct SubtractAction { a: f64, b: f64 }

impl SubtractAction {
    async fn run(&self) -> Result<String, ToolError> {
        Ok(format!("{} - {} = {}", self.a, self.b, self.a - self.b))
    }
}

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![
            AddAction::default().definition(),
            SubtractAction::default().definition(),
        ]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddAction::TOOL_NAME => AddAction::default().execute(arguments).await,
            SubtractAction::TOOL_NAME => SubtractAction::default().execute(arguments).await,
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

## ToolProvider Adapters

`ene-tool-common` provides two adapters that implement `ToolProvider` for you, so most tools never need to hand-write the `list_specs`/`call_tool` dispatch loop:

| Adapter | Use for | Session/sandbox hooks |
|---|---|---|
| `ActionSetProvider::new(vec![...])` | One or more actions per binary (the mega-tool pattern used by `ene-tool-fs`, `ene-tool-app`, `ene-tool-browser`) | `.with_set_call_context_hook(...)`, `.with_sandbox_hook(...)` |

Both dispatch `call_tool` by matching `ToolAction::name()` against the requested tool name and return `ToolError::NotFound` on a miss — the same behavior every hand-written provider in this codebase used to reimplement. If an action needs to react to `set_call_context`/`set_sandbox` (e.g. to thread a conversation ID or a DB socket into shared state), register a hook instead of dropping down to a manual `ToolProvider` impl:

```rust
use ene_tool_common::ActionSetProvider;
use std::sync::Arc;

let state = Arc::new(MyState::default());
let session_state = state.clone();

let provider = ActionSetProvider::new(vec![Box::new(MyAction::new(state))])
    .with_set_call_context_hook(move |conv_id| session_state.set_session_id(conv_id));
```

See `tools/utility/src/provider.rs` for a full worked example (session ID + DB sandbox socket/token both threaded through hooks).

## ToolProvider Trait Reference

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tool specs this provider exposes.
    /// Mega-tools return N specs, one per action (e.g. `filesystem.read`, ...).
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// Returns per-action metadata (used for Tool RAG embedding).
    /// For individual tools, this returns an empty vec.
    fn list_action_specs(&self) -> Vec<ActionSpec> { Vec::new() }

    /// Executes a tool by name with the given JSON arguments.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// Sets the current session ID (for undo tracking, session-scoped state, etc.).
    fn set_session_id(&self, session_id: &str);

    /// Receives sandbox configuration (for filesystem tools; default: no-op).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Receives tool-specific configuration (called once during Initialize).
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Returns the tool's current configuration.
    fn get_config(&self) -> serde_json::Value { serde_json::Value::Null }

    /// Returns the JSON Schema for the configuration this tool accepts.
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolSpec

The structured, LLM-facing tool specification (slimmed at v3):

```rust
pub struct ToolSpec {
    pub name: ToolName,                     // e.g. "filesystem.read"
    pub description: String,                // full markdown description (used for RAG embedding)
    pub parameters: serde_json::Value,      // JSON Schema (auto-derived from schemars)
    /// Negative keywords for RAG disambiguation — when present in the
    /// user query, these terms *penalize* the tool's relevance score.
    /// Interim field re-instated after #135 slim-down until
    /// `ToolRagProfile` lands (#137). Wire-invisible when empty
    /// (`skip_serializing_if = "Vec::is_empty"`).
    pub negative_keywords: Vec<String>,
}
```

The `negative_keywords` field is a temporary vestige of the pre-#135 fat `ToolSpec`. It is retained only so that `#[tool(keywords_negative = "...")]` authoring data in derive macros is not lost before `ToolRagProfile` ships (#137). On the wire, an empty `Vec` is skipped entirely.

## `#[tool(...)]` Attributes

```rust
#[derive(ToolAction, JsonSchema, Deserialize)]
#[tool(
    // Required
    namespace = "calculator",        // Namespace prefix
    name = "add",                    // Action name (full: "calculator.add")
    summary = "Add two numbers.",    // One-line summary (embedding field)
    category = "Utility",            // ToolCategory variant

    // Optional
    display_name = "Add Numbers",    // Defaults to Title-Case of name
    description = "Longer markdown", // Defaults to summary
    version = "1.0.0",               // Defaults to 1.0.0
    side_effects = "ReadOnly",       // Defaults to ReadOnly

    // Keywords (comma-separated)
    keywords_primary = "add, sum, plus",
    keywords_secondary = "math, number",
    keywords_domain = "arithmetic",
    keywords_negative = "subtract, remove",

    // Metadata (comma-separated)
    caveats = "Division by zero returns an error.",
    preconditions = "Arguments must be valid numbers.",
    related = "calculator.subtract, calculator.multiply",

    // Examples (semicolon-separated, each: description|input|output)
    examples = "Add 2 and 3|{ \"a\": 2, \"b\": 3 }|2 + 3 = 5"
)]
pub struct AddAction {
    /// First operand.
    a: f64,
    /// Second operand.
    b: f64,
}
```

See [derive-macro.md](derive-macro.md) for the full attribute reference.

## ToolCategory

| Variant | Use for |
|---------|---------|
| `Filesystem` | File read, write, edit operations |
| `Shell` | Shell command execution |
| `Browser` | Web fetching, browser automation |
| `App` | GUI automation, desktop interaction |
| `WebSearch` | Search engine queries |
| `WebFetch` | URL fetching |
| `Utility` | Helper tools (time, system info, etc.) |
| `Memory` | Long-term memory operations |
| `Search` | Local search / RAG over user documents |
| `Meta` | Self-introspection, tool selection |

## ToolError

```rust
pub enum ToolError {
    NotFound { tool_name: String },
    InvalidArguments { message: String },
    ExecutionFailed { message: String },
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    IoError { message: String },
    Timeout { message: String },
    Internal { message: String },
    IpcTransport { message: String },
    PermissionRequired { request_id: String, action: String, target: String, description: String },
    UserInputRequired { request_id: String, prompt: UserInputPrompt },
    FileNotFound { path: String },
    FileTooLarge { path: String, size: u64, limit: u64 },
    CommandBlocked { command: String, reason: String },
    ShellTimeout { command: String, timeout_ms: u64 },
    ShellOutputTooLarge { size: u64, limit: u64 },
    BrowserError { message: String },
    AppError { message: String },
    WebSearchError { message: String },
    IpcClient { message: String },
    Other { message: String },
}
```

## IPC Lifecycle

```
Tool binary starts
  → listens on ENE_TOOL_SOCKET (provided by ToolHostManager as env var)
  → receives IpcRequest::Handshake → responds HandshakeAck
  → Handshake carries sandbox + tool_config (Initialize folded at v3)
  → ready to handle CallTool requests
```

## Protocol Variants

The IPC wire protocol at `IPC_PROTOCOL_VERSION = 3` carries 10 request variants and 8 response variants. `UserInput` is **not** an IPC variant — it is surfaced through `ToolError::UserInputRequired` and handled by `ene-runtime`'s streaming loop.

### Requests (host → tool)

| Variant | Payload | Semantics | Since |
|---|---|---|---|
| `Handshake` | `version: u32`, `sandbox: SandboxConfigData`, `tool_config: Option<Value>` | Protocol negotiation + sandbox config + tool config push (Initialize folded at v3) | v1 |
| `ListTools` | — | Fetch all `ToolSpec`s from the provider | v1 |
| `ListActionSpecs` | — | Fetch per-action specs (mega-tool capability metadata for RAG) | v2 |
| `GetConfigSchema` | — | Request the tool's config JSON Schema (#150 exception, not part of the "six primary") | v2 |
| `CallTool` | `name: String`, `arguments: String` | Execute a tool by name with JSON arguments | v1 |
| `SetCallContext` | `conversation_id: String`, `turn_id: String` | Thread conversation + turn identifiers into the tool (supersedes v2 `SetSessionId`) | v2 |
| `ApprovePermission` | `request_id: String` | Approve a pending destructive-operation permission request | v1 |
| `AllowPattern` | `action: String`, `target_pattern: String` | Register a session-wide permission allow pattern (action + target glob) | v1 |
| `Ping` | — | Health-check ping for liveness monitoring | v2 |
| `Shutdown` | — | Graceful shutdown request | v1 |

### Responses (tool → host)

| Variant | Payload | Triggered by |
|---|---|---|
| `HandshakeAck` | `version: u32` | `Handshake` |
| `Ack` | — | `SetCallContext`, `ApprovePermission`, `AllowPattern`, `Shutdown` |
| `Tools` | `tools: Vec<ToolSpec>` | `ListTools` |
| `ActionSpecs` | `specs: Vec<ActionSpec>` | `ListActionSpecs` |
| `ConfigSchema` | `schema: Option<Value>` | `GetConfigSchema` |
| `CallResult` | `result: Result<String, ToolError>` | `CallTool` |
| `Pong` | — | `Ping` |
| `Error` | `error: String` | Any request that fails at the IPC level |

## ABI Compatibility

The wire ABI is everything in `ene-tool-proto`'s IPC surface: `IpcRequest`/`IpcResponse`, `IPC_PROTOCOL_VERSION`, `ToolSpec`/`ActionSpec` fields, `SandboxConfigData`, and `ToolError`. `run_tool_server` **strictly rejects** a handshake with a mismatched `IPC_PROTOCOL_VERSION` — there is no downgrade/negotiation — so version-bump decisions matter.

| Change | Compatible? | Action required |
|---|---|---|
| Add a new `IpcRequest`/`IpcResponse` enum variant | ✅ Additive | None — old tool binaries simply never send/receive the new variant. New host code must still handle old tool binaries not sending it. |
| Add a new optional field to `ToolSpec`/`ActionSpec`/`SandboxConfigData` (with a `#[serde(default)]` or macro-provided default) | ✅ Additive | None. Follow the `define_tool_config!`/`schemars` pattern already used by `SandboxConfigData` so old JSON without the field still deserializes. |
| Add a new `ToolError` variant | ✅ Additive | None — `ToolError` is `#[serde(tag = "kind" ...)]`-free (plain enum), so new variants deserialize fine as long as old code doesn't exhaustively `match` without a wildcard arm. Prefer adding a `_ => ...` arm in new `match`es over new crates. |
| Add a new `ToolProvider` trait method | ✅ Additive, if it has a default impl | Give it a default (no-op / empty) implementation, exactly like `set_sandbox`, `approve_permission`, `set_config`, etc. already do — this is what keeps every existing provider (hand-written or adapter-based) compiling. |
| Remove/rename an existing `IpcRequest`/`IpcResponse` variant, or change a field's type/meaning | ❌ Breaking | Bump `IPC_PROTOCOL_VERSION` in `ene-tool-proto` (see [AGENTS.md §6 R3](../../AGENTS.md)). Update both the host (`ene-tool-host`) and every tool binary in the same change. |
| Remove a `ToolProvider` trait method, or make an existing default-having method required | ❌ Breaking | Same as above — this changes what every tool binary must implement. Requires a coordinated update across `tools/*` plus a `PROTOCOL_VERSION` bump if it also changes wire behavior. |
| Change `run_tool_server`'s signature in a way that breaks `Box::new(provider)` call sites | ❌ Breaking (source-level) | Does not require a `PROTOCOL_VERSION` bump on its own (it's a Rust API break, not a wire break), but update every `tools/*/src/main.rs` call site and the recipe in `AGENTS.md` §6 R1 in the same change. |

In short: **additive is always safe**; anything that changes the meaning of an existing wire field/variant or removes something a tool binary might already be sending needs a `PROTOCOL_VERSION` bump plus a coordinated host+tool-binary update.

## Best Practices

1. **Use `#[derive(ToolAction)]`** — One derive generates the spec, schema, and dispatch. Write `async fn run(&self)` for your logic.
2. **Use `#[tool(skip)]` for dependencies** — Sandbox, database, HTTP client, etc. are hidden from the LLM and injected by the provider.
3. **Use namespaces** — Group related tools under a namespace (e.g. `calculator.add`, `calculator.subtract`).
4. **Write good summaries** — The summary is the primary embedding field for Tool RAG. Be clear about when and how to use the tool.
5. **Use keywords** — Include synonyms and related terms. Primary keywords are weighted highest in RAG scoring.
6. **Set side effects correctly** — This helps the LLM understand tool safety and helps the sandbox make correct decisions.
7. **Handle errors gracefully** — Return `ToolError` variants with clear messages. The LLM may try to correct its usage based on error messages.
