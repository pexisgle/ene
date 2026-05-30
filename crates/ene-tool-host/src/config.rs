use serde::de::DeserializeOwned;
use std::collections::HashMap;

fn default_tools() -> HashMap<String, ToolEntry> {
    ["fs", "web", "browser", "utility", "app"]
        .into_iter()
        .map(|name| (name.to_string(), ToolEntry::default()))
        .collect()
}

ene_config::define_config!(
    "tools",
    /// Global tool subsystem config.
    pub struct ToolConfig {
        /// Whether tool calling is enabled globally.
        pub tool_calling_enabled: bool = true,
        /// Maximum number of sequential tool calls per turn.
        pub max_tool_call_rounds: usize = 10,
        /// Tool call execution timeout in milliseconds.
        pub tool_call_timeout_ms: u64 = 60_000,
        /// Per-tool enable/disable map with optional extra config.
        pub tools: HashMap<String, ToolEntry> = default_tools(),
    }
);

ene_config::define_config!(
    "tool_entry",
    /// A single tool entry in the `tools` map.
    pub struct ToolEntry {
        /// Whether this tool is enabled.
        pub enable: bool = true,
        /// Tool-specific configuration (flattened into the parent).
        #[serde(flatten)]
        pub config: serde_json::Value = serde_json::Value::Object(Default::default()),
    }
);

impl ToolEntry {
    /// Deserializes tool-specific settings in a type-safe manner and supports type completion
    pub fn deserialize_config<T>(&self) -> Result<T, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(self.config.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[schemars(crate = "::ene_config::schemars")]
/// Configuration for an MCP server connection.
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
