use serde::de::DeserializeOwned;
use std::collections::HashMap;

const fn default_max_rounds() -> usize {
    10
}

const fn default_timeout_ms() -> u64 {
    60_000
}

fn default_tools() -> HashMap<String, ToolEntry> {
    ["fs", "web", "browser", "utility", "app"]
        .into_iter()
        .map(|name| (name.to_string(), ToolEntry::default()))
        .collect()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
/// A single tool entry in the `tools` map.
pub struct ToolEntry {
    /// Whether this tool is enabled.
    pub enable: bool,
    /// Tool-specific configuration (flattened into the parent).
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl Default for ToolEntry {
    fn default() -> Self {
        Self {
            enable: true,
            config: serde_json::Value::Object(serde_json::Map::default()),
        }
    }
}

ene_config::define_config!(
    settings,
    "tools",
    /// Global tool subsystem config.
    pub struct ToolConfig {
        /// Whether tool calling is enabled globally.
        pub enabled: bool = true,
        /// Maximum number of sequential tool calls per turn.
        #[serde(skip_deserializing, default = "default_max_rounds", skip_serializing)]
        #[schemars(skip)]
        pub max_rounds: usize = default_max_rounds(),
        /// Tool call execution timeout in milliseconds.
        #[serde(skip_deserializing, default = "default_timeout_ms", skip_serializing)]
        #[schemars(skip)]
        pub timeout_ms: u64 = default_timeout_ms(),
        /// Per-tool enable/disable map with optional extra config.
        pub list: HashMap<String, ToolEntry> = default_tools(),
        /// MCP servers.
        pub mcp_servers: Vec<crate::mcp_config::McpServerConfig>,
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
