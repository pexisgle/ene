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
    /// Connect via HTTP.
    Http {
        /// Server URL.
        url: String,
    },
}
