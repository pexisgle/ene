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
ene-tool-proto = { git = "https://github.com/pexisgle/Ene" }
ene-tool-derive = { git = "https://github.com/pexisgle/Ene" }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "1"
```

### 2. Define Args Structs with `#[derive(ToolSpec)]`

Each tool has a typed args struct. The derive macro generates a `spec() -> ToolSpec` method with auto-generated JSON Schema (via `schemars`), and a `TOOL_NAME` constant for dispatch.

```rust
use ene_tool_derive::ToolSpec;
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(
    namespace = "greeter",
    name = "hello",
    summary = "Returns a greeting for the given name.",
    category = "Utility",
    keywords_primary = "greeting, hello",
    side_effects = "ReadOnly"
)]
pub struct HelloArgs {
    /// Name to greet.
    pub name: String,
}
```

The derive macro generates:
- `HelloArgs::TOOL_NAME` = `"greeter.hello"`
- `HelloArgs::spec()` → full `ToolSpec` with auto-generated JSON Schema from `schemars`

### 3. Implement ToolProvider

```rust
use ene_tool_proto::{ToolError, ToolProvider, ToolSpec, run_tool_server};
use async_trait::async_trait;

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![HelloArgs::spec()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            HelloArgs::TOOL_NAME => {
                let args: HelloArgs = serde_json::from_str(arguments)
                    .map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                Ok(format!("Hello, {}!", args.name))
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

## Multiple Tools

Expose multiple tools from a single provider by returning multiple specs:

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "add", summary = "Add two numbers.", category = "Utility")]
pub struct AddArgs { pub a: f64, pub b: f64 }

#[derive(ToolSpec, JsonSchema, Deserialize)]
#[tool(namespace = "calculator", name = "subtract", summary = "Subtract two numbers.", category = "Utility")]
pub struct SubtractArgs { pub a: f64, pub b: f64 }

struct CalculatorProvider;

#[async_trait]
impl ToolProvider for CalculatorProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        vec![AddArgs::spec(), SubtractArgs::spec()]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            AddArgs::TOOL_NAME => {
                let args: AddArgs = serde_json::from_str(arguments)?;
                Ok(format!("{} + {} = {}", args.a, args.b, args.a + args.b))
            }
            SubtractArgs::TOOL_NAME => {
                let args: SubtractArgs = serde_json::from_str(arguments)?;
                Ok(format!("{} - {} = {}", args.a, args.b, args.a - args.b))
            }
            _ => Err(ToolError::NotFound { tool_name: name.to_string() }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

Each tool is a first-class `ToolSpec` with its own typed args, auto-generated JSON Schema, and rich metadata for the Tool RAG pipeline.

## ToolProvider Trait Reference

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tool specs this provider exposes.
    fn list_specs(&self) -> Vec<ToolSpec>;

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

## `#[derive(ToolSpec)]` Attributes

```rust
#[derive(ToolSpec, JsonSchema, Deserialize)]
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
pub struct AddArgs {
    /// First operand.
    pub a: f64,
    /// Second operand.
    pub b: f64,
}
```

See [derive-macro.md](derive-macro.md) for the full attribute reference.

## ToolName

A validated, namespaced tool identifier:

```rust
ToolName::new("filesystem.read")  // namespace = "filesystem", action = "read"
ToolName::new("get_current_time") // no namespace
```

## KeywordSet

Structured keywords with weighted tiers for Tool RAG:

```rust
KeywordSet {
    primary: vec!["read", "open", "cat"],      // weight 1.0
    secondary: vec!["file", "filesystem"],      // weight 0.6
    domain: vec!["linux", "posix"],             // weight 0.3
    negative: vec!["write", "delete"],          // weight -0.5 (penalty)
}
```

## SideEffects

```rust
pub enum SideEffects {
    ReadOnly,                           // No side effects
    FileSystem { mutates: bool },       // File I/O
    Network { external: bool },         // Network access
    System { privileged: bool },        // Process spawn, signals
    Browser { mutates_dom: bool },      // Browser automation
    Destructive,                        // Data loss possible
    Idempotent,                         // Safe to retry
}
```

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
pub type ToolError = EneToolProtoError;

pub enum EneToolProtoError {
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
}
```

## IPC Lifecycle

```
Tool binary starts
  → listens on ENE_TOOL_SOCKET (provided by ToolHostManager as env var)
  → receives IpcRequest::Initialize
  → tool initialized with sandbox + config
  → ready to handle CallTool requests
```

## Best Practices

1. **One args struct per tool** — Each tool gets its own `#[derive(ToolSpec)]` struct. This gives you typed args, auto-generated JSON Schema, and a `TOOL_NAME` constant for dispatch.
2. **Use namespaces** — Group related tools under a namespace (e.g. `calculator.add`, `calculator.subtract`).
3. **Write good summaries** — The summary is the primary embedding field for Tool RAG. Be clear about when and how to use the tool.
4. **Use keywords** — Include synonyms and related terms. Primary keywords are weighted highest in RAG scoring.
5. **Set side effects correctly** — This helps the LLM understand tool safety and helps the sandbox make correct decisions.
6. **Handle errors gracefully** — Return `ToolError` variants with clear messages. The LLM may try to correct its usage based on error messages.
