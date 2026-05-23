pub mod error;
pub mod ipc_client;
pub mod mcp_client;
pub mod tool_host_manager;
pub mod tools;

pub use error::ToolError;
pub use ipc_client::IpcToolRegistry;
pub use mcp_client::McpToolRegistry;
pub use tool_host_manager::ToolHostManager;
pub use tools::{
    CompositeToolRegistry, ToolCategory, ToolDefinition, ToolRegistry, compute_tool_version_hash,
};
