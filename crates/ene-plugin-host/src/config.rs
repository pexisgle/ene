//! Plugin system configuration section.

use std::collections::HashMap;

const fn default_max_rounds() -> usize {
    10
}

const fn default_timeout_ms() -> u64 {
    60_000
}

const fn default_health_interval_ms() -> u64 {
    30_000
}

/// Default plugin list containing the builtin tool plugins.
fn default_plugin_list() -> HashMap<String, PluginEntry> {
    ["app", "browser", "fs", "utility", "web"]
        .into_iter()
        .map(|name| (name.to_string(), PluginEntry::default()))
        .collect()
}

/// A single plugin entry in the `plugins.list` map.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PluginEntry {
    /// Whether this plugin is enabled.
    pub enable: bool,
    /// Expected SHA-256 checksum of the plugin binary (hex-encoded).
    /// When set, the binary is verified before launch.
    /// When absent, a one-time warning is logged on first launch.
    #[serde(default)]
    pub checksum: Option<String>,
    /// Plugin-specific configuration (flattened into the parent).
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self {
            enable: true,
            checksum: None,
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
        #[serde(default = "default_plugin_list")]
        pub list: HashMap<String, PluginEntry> = default_plugin_list(),
        /// Maximum number of concurrent tool calls.
        pub max_concurrent: usize = 8,
        /// Maximum number of sequential tool calls per turn.
        #[serde(default = "default_max_rounds")]
        pub max_rounds: usize = default_max_rounds(),
        /// Tool call execution timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        pub timeout_ms: u64 = default_timeout_ms(),
        /// Interval between health probe pings in milliseconds.
        ///
        /// Set to `0` to disable periodic health checks.
        #[serde(default = "default_health_interval_ms")]
        pub health_interval_ms: u64 = default_health_interval_ms(),
        /// MCP servers to connect to.
        pub mcp_servers: Vec<crate::mcp_config::McpServerConfig> = Vec::new(),
    }
);
