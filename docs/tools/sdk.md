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
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

### 2. Implement ToolProvider

```rust
// src/main.rs
use ene_tool_proto::{
    ToolProvider, ToolDefinition, ToolCategory, ToolError,
    SandboxConfigData, run_tool_server,
};
use async_trait::async_trait;

struct MyToolProvider;

#[async_trait]
impl ToolProvider for MyToolProvider {
    fn list_tools(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "hello".into(),
            description: "Returns a greeting for the given name.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name to greet"
                    }
                },
                "required": ["name"]
            }),
            category: Some(ToolCategory::Utility),
            keywords: vec!["greeting".into(), "hello".into()],
        }]
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        match name {
            "hello" => {
                let args: serde_json::Value = serde_json::from_str(arguments)
                    .map_err(|e| ToolError::InvalidArguments {
                        message: e.to_string(),
                    })?;
                let name = args["name"].as_str().unwrap_or("world");
                Ok(format!("Hello, {}!", name))
            }
            _ => Err(ToolError::NotFound {
                tool_name: name.to_string(),
            }),
        }
    }

    fn set_session_id(&self, _session_id: &str) {}
}
```

### 3. Start the Server

```rust
#[tokio::main]
async fn main() {
    run_tool_server(Box::new(MyToolProvider)).await.unwrap();
}
```

### 4. Build and Deploy

```bash
cargo build --release
mkdir -p ~/.local/share/dev.pexisgle.ene/tools
cp target/release/my-cool-tool ~/.local/share/dev.pexisgle.ene/tools/
```

### 5. Enable in Settings

```json
{
  "tools": {
    "tools": {
      "my-cool-tool": { "enable": true }
    }
  }
}
```

## ToolProvider Trait Reference

```rust
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Returns the list of tools this provider exposes.
    fn list_tools(&self) -> Vec<ToolDefinition>;

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

## ToolDefinition

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,  // JSON Schema for parameters
    pub category: Option<ToolCategory>,
    pub keywords: Vec<String>,
}
```

### ToolCategory

| Variant | Use for |
|---------|---------|
| `Filesystem` | File read, write, edit operations |
| `Shell` | Shell command execution |
| `Browser` | Web fetching, browser automation |
| `WebSearch` | Search engine queries |
| `App` | GUI automation, desktop interaction |
| `Utility` | Helper tools (time, system info, etc.) |

## ToolError

The error type in `ene-tool-proto` is `EneToolProtoError`, with `ToolError` as a type alias:

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

**Note:** `ene-tool-host` has its own separate `ToolError` type with additional domain-specific variants (`FileNotFound`, `FileTooLarge`, `CommandBlocked`, `ShellTimeout`, `BrowserError`, `AppError`, `IpcClient`, etc.). The host-side error is mapped to the proto-side error at the IPC boundary.

## IPC Lifecycle

```
Tool binary starts
  → listens on ENE_TOOL_SOCKET (provided by ToolHostManager as env var)
  → receives IpcRequest::Initialize
  → tool initialized with sandbox + config
  → ready to handle CallTool requests
```

## Best Practices

1. **Make descriptions LLM-friendly** — The description is what the LLM reads to decide when to call your tool. Be clear about when and how to use it.
2. **Use JSON Schema properly** — Parameters are validated by the LLM's function calling. Use `required`, `type`, `description`, and `enum` constraints.
3. **Keywords matter** — Keywords feed Tool RAG embeddings. Include synonyms and related terms.
4. **Handle errors gracefully** — Return `ToolError` variants with clear messages. The LLM may try to correct its usage based on error messages.
5. **Session isolation** — If your tool maintains session state, use `set_session_id()` to scope it.
6. **Config schemas** — Implement `config_schema()` if your tool has configurable settings. This feeds into the auto-generated `settings.schema.json`.
