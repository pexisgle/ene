use std::collections::HashMap;
use serde::de::DeserializeOwned;

fn default_tools() -> HashMap<String, ToolEntry> {
    ["fs", "web", "browser", "utility", "app"]
        .into_iter()
        .map(|name| (name.to_string(), ToolEntry::default()))
        .collect()
}

ene_config::define_config!(
    "tools",
    pub struct ToolSettings {
        pub tool_calling_enabled: bool = true,
        pub max_tool_call_rounds: usize = 10,
        pub tools: HashMap<String, ToolEntry> = default_tools(),
    }
);

ene_config::define_config!(
    "tool_entry",
    pub struct ToolEntry {
        pub enable: bool = true,
        #[serde(flatten)]
        pub config: serde_json::Value = serde_json::Value::Object(Default::default()),
    }
);

impl ToolEntry {
    /// ツール固有の設定を型安全にデシリアライズして型補完をサポートします。
    pub fn deserialize_config<T>(&self) -> Result<T, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_value(self.config.clone())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[schemars(crate = "::ene_config::schemars")]
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum McpTransport {
    Stdio { command: String, args: Vec<String> },
    Http { url: String },
}
