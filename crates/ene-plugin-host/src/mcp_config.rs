//! MCP server configuration types.

/// Configuration for an MCP server connection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[schemars(crate = "::ene_config::schemars")]
pub struct McpServerConfig {
    /// Server name (used for display and routing).
    pub name: String,
    /// Whether this MCP server is enabled.
    pub enabled: bool,
    /// Transport configuration.
    pub transport: McpTransport,
    /// Environment variable names to pass through from the host process
    /// to the MCP server child process (stdio transport only). All other
    /// inherited environment variables are cleared for security.
    ///
    /// Many MCP servers are launched via `npx`/`uvx` wrappers that expect
    /// inherited env (API keys, `NODE_PATH`, virtualenv paths). This field
    /// provides an explicit opt-in escape hatch, mirroring the plugin
    /// `env_passthrough` mechanism.
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    /// OS sandbox settings for the child process (stdio transport only).
    /// `None` leaves the server unsandboxed.
    #[serde(default)]
    pub sandbox: Option<crate::config::SandboxEntryConfig>,
}

/// MCP server transport type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum McpTransport {
    /// Spawn a child process with stdio transport.
    Stdio {
        /// The command to run.
        command: String,
        /// Arguments for the command.
        args: Vec<String>,
    },
    /// Connect via HTTP (SSE).
    Http {
        /// Server URL.
        url: String,
        /// Optional HTTP header for authentication (e.g., "Bearer <token>").
        #[serde(default)]
        auth_header: Option<String>,
    },
}
