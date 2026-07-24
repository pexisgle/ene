//! Plugin system configuration section.

use std::collections::HashMap;

const fn default_max_rounds() -> usize {
    10
}

const fn default_timeout_ms() -> u64 {
    60_000
}

/// A single plugin entry in the `plugins.list` map.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PluginEntry {
    /// Whether this plugin is enabled.
    pub enable: bool,
    /// Plugin-specific configuration (flattened into the parent).
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self {
            enable: true,
            config: serde_json::Value::Object(serde_json::Map::default()),
        }
    }
}

ene_config::define_config!(
    settings,
    "plugins",
    /// Plugin system configuration.
    pub struct PluginConfig {
        /// Enable the plugin system.
        pub enabled: bool = true,
        /// Named plugin entries (tools and providers).
        pub list: HashMap<String, PluginEntry> = HashMap::new(),
        /// Maximum number of concurrent tool calls.
        pub max_concurrent: usize = 8,
        /// Maximum number of sequential tool calls per turn.
        #[serde(skip_deserializing, default = "default_max_rounds", skip_serializing)]
        #[schemars(skip)]
        pub max_rounds: usize = default_max_rounds(),
        /// Tool call execution timeout in milliseconds.
        #[serde(skip_deserializing, default = "default_timeout_ms", skip_serializing)]
        #[schemars(skip)]
        pub timeout_ms: u64 = default_timeout_ms(),
        /// MCP servers to connect to.
        pub mcp_servers: Vec<crate::mcp_config::McpServerConfig> = Vec::new(),
    }
);
