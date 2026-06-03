# Security Sandbox

The sandbox system confines tool operations to a configured set of directories and restricts dangerous shell commands.

## Configuration Delivery

1. `SandboxConfigData` is created from `settings.json` → `sandbox` section
2. Sent to each tool binary via `IpcRequest::Initialize { sandbox, tool_config }`
3. Tool-side `Sandbox` struct enforces all access controls

## SandboxConfigData

```rust
pub struct SandboxConfigData {
    pub enabled: bool,
    pub allowed_directories: Vec<String>,     // Read-allowed paths
    pub writable_directories: Vec<String>,    // Write-allowed paths
    pub blocked_commands: Vec<String>,        // Blocked command regex patterns
    pub max_read_bytes: usize,                // 50KB default
    pub max_write_bytes: usize,               // 1MB default
    pub shell_timeout_ms: u64,                // 120s default
    pub max_shell_output_bytes: usize,        // 50KB default
    pub max_shell_output_lines: usize,        // 2000 default
    pub undo_db_path: Option<String>,
}
```

## Check Flow

```
File/Shell operation request
  ↓
Sandbox enabled?
  ├── No → Direct execution
  └── Yes
       ├── Path normalization (read/write only)
       ├── allowed_directories / writable_directories? → No → Rejected
       ├── Shell: blocked_commands pattern match? → Yes → Rejected
       └── Execute with size/output limits
```

## Blocked Command Patterns

Default patterns that ship with the sandbox:

| Pattern | Target |
|---------|--------|
| `rm\s+-rf\s+/` | Root filesystem deletion |
| `dd\s+if=` | Disk destruction |
| `mkfs` | Filesystem formatting |
| `sudo\s+` | Privilege escalation |
| `:\s*\{\s*\|\s*&\s*;\s*\}` | Fork bombs |

## Undo System

The `Sandbox` maintains an undo stack for all file modifications:

| Method | Description |
|--------|-------------|
| `track_overwrite(path, content)` | Saves original content before overwrite |
| `track_creation(path)` | Records creation (undo deletes) |
| `track_deletion(path, content)` | Saves content before deletion |
| `track_patch(entries)` | Groups all patch changes as one undo entry |
| `undo_last()` | Rolls back the most recent operation |

Undo is backed by a SQLite database with zlib compression (`undodb_path`/`undo.db`).

## Error Types

Sandbox violations return `ToolError::SandboxViolation { message }` from `ene-tool-proto`. This is a unified error type shared across all tool crates — no boundary mapping is required.

```rust
pub enum ToolError {
    SandboxViolation { message: String },
    PermissionDenied { message: String },
    PermissionRequired { request_id: String, action: String, target: String, description: String },
    // ... other variants
}
```

When a destructive operation requires user approval, the tool returns `ToolError::PermissionRequired` with a `request_id` that can be approved via `ToolProvider::approve_permission()`.
