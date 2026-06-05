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
ene-tool-common = { git = "https://github.com/pexisgle/Ene" }
ene-tool-proto = { git = "https://github.com/pexisgle/Ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/Ene" }
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

Expose multiple tools from a single provider:

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

## ToolProvider Trait Reference

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tool specs this provider exposes.
    fn list_specs(&self) -> Vec<ToolSpec>;

    /// Returns per-action metadata for mega-tools (default: empty).
    fn list_action_specs(&self) -> Vec<ActionSpec> { vec![] }

    /// Executes a tool by name with the given JSON arguments.
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError>;

    /// Called when the session ID changes (for session-scoped state).
    fn set_session_id(&self, session_id: &str);

    /// Receives sandbox configuration (for filesystem tools).
    fn set_sandbox(&self, _sandbox: &SandboxConfigData) {}

    /// Approves a pending destructive-operation permission request by ID.
    fn approve_permission(&self, _request_id: &str) {}

    /// Adds a session-wide permission allow pattern (action + target glob).
    fn allow_pattern(&self, _action: &str, _target_pattern: &str) {}

    /// Receives tool-specific config from settings.json.
    fn set_config(&self, _config: &serde_json::Value) {}

    /// Returns JSON Schema for the config this tool accepts.
    fn config_schema(&self) -> Option<serde_json::Value> { None }
}
```

## ToolSpec

The structured, LLM-facing tool specification:

```rust
pub struct ToolSpec {
    pub name: ToolName,           // e.g. "filesystem.read"
    pub version: ToolVersion,     // semver (1.0.0)
    pub display_name: String,     // "Read File"
    pub summary: String,          // one-line, used for embedding
    pub description: String,      // full markdown
    pub category: ToolCategory,   // Filesystem, Utility, etc.
    pub keywords: KeywordSet,     // structured keyword bag
    pub parameters: serde_json::Value,  // JSON Schema (auto from schemars)
    pub examples: Vec<ToolExample>,
    pub caveats: Vec<String>,
    pub side_effects: SideEffects,
    pub preconditions: Vec<String>,
    pub related: Vec<ToolName>,
}
```

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
  → receives IpcRequest::Initialize
  → tool initialized with sandbox + config
  → ready to handle CallTool requests
```

## Best Practices

1. **Use `#[derive(ToolAction)]`** — One derive generates the spec, schema, and dispatch. Write `async fn run(&self)` for your logic.
2. **Use `#[tool(skip)]` for dependencies** — Sandbox, database, HTTP client, etc. are hidden from the LLM and injected by the provider.
3. **Use namespaces** — Group related tools under a namespace (e.g. `calculator.add`, `calculator.subtract`).
4. **Write good summaries** — The summary is the primary embedding field for Tool RAG. Be clear about when and how to use the tool.
5. **Use keywords** — Include synonyms and related terms. Primary keywords are weighted highest in RAG scoring.
6. **Set side effects correctly** — This helps the LLM understand tool safety and helps the sandbox make correct decisions.
7. **Handle errors gracefully** — Return `ToolError` variants with clear messages. The LLM may try to correct its usage based on error messages.
