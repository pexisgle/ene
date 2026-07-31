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

const fn default_handshake_timeout_ms() -> u64 {
    10_000
}

const fn default_permission_prompt_timeout_ms() -> u64 {
    300_000
}

const fn default_user_input_prompt_timeout_ms() -> u64 {
    600_000
}

/// Default plugin list containing the builtin tool and provider plugins.
fn default_plugin_list() -> HashMap<String, PluginEntry> {
    let mut list: HashMap<String, PluginEntry> = ["app", "browser", "fs", "utility", "web"]
        .into_iter()
        .map(|name| (name.to_string(), PluginEntry::default()))
        .collect();

    // The Anthropic provider plugin needs ANTHROPIC_API_KEY forwarded from
    // the host environment; without it the provider cannot authenticate.
    list.insert(
        "anthropic".to_string(),
        PluginEntry {
            env_passthrough: vec!["ANTHROPIC_API_KEY".to_string()],
            ..PluginEntry::default()
        },
    );

    list
}

/// A single plugin entry in the `plugins.list` map.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PluginEntry {
    /// Whether this plugin is enabled.
    pub enable: bool,
    /// Expected SHA-256 checksum of the plugin binary (hex-encoded).
    /// When set, the binary is verified before launch.
    /// When absent, the checksum is computed on first activation and
    /// recorded back to configuration (trust-on-first-use).
    #[serde(default)]
    pub checksum: Option<String>,
    /// Environment variable names to pass through from the host process
    /// to the plugin child process. All other inherited environment
    /// variables are cleared for security (`env_clear()`).
    ///
    /// This is an interim mechanism until a proper credential service
    /// is implemented (#412/#413).
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    /// Plugin-specific configuration (flattened into the parent).
    #[serde(flatten)]
    pub config: serde_json::Value,
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self {
            enable: true,
            checksum: None,
            env_passthrough: Vec::new(),
            config: serde_json::Value::Object(serde_json::Map::default()),
        }
    }
}

// Re-register the `fs` sandbox tool schema that was previously emitted by
// `define_tool_config!` inside `ene-plugin-proto`. The proto crate is wire-ABI
// only and no longer depends on `ene-config`, so the host crate (which links
// both) takes over the registration.
const _: () = {
    /// # Safety
    ///
    /// Called by `ctor` before `main`. Only safe registration code
    /// is executed; no I/O, TLS, or cross-ctor ordering assumed.
    #[ene_config::ctor(unsafe, crate_path = ene_config)]
    fn register_fs_sandbox_schema() {
        ene_config::register_tool_schema::<ene_plugin_proto::SandboxConfigData>("fs");
    }
};

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
        /// Maximum number of concurrent in-flight IPC requests per plugin
        /// connection.
        ///
        /// This is a per-connection bound over *all* request types — tool
        /// calls, pings, `list_tools`, `chat_completion`, and so on — not just
        /// tool calls. Requests beyond the bound queue (bounded by their own
        /// timeout) rather than fanning out to the plugin. Chat *streams*
        /// (`CreateChatStream`) are the exception: they bypass this bound and
        /// are not counted against it.
        pub max_concurrent: usize = 8,
        /// Maximum number of sequential tool calls per turn.
        #[serde(default = "default_max_rounds")]
        pub max_rounds: usize = default_max_rounds(),
        /// Tool call execution timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        pub timeout_ms: u64 = default_timeout_ms(),
        /// How long the runtime waits for a consumer to answer a tool
        /// *permission* prompt before failing safe (#401).
        ///
        /// The wait is bounded by this timeout **and** selected against the
        /// turn's cancel token, so a consumer that never responds (a lost
        /// event, a headless/automation consumer, a closed window) cannot
        /// hold the turn open forever: on expiry the prompt is treated as
        /// denied and the turn still reaches `Terminal`, releasing the turn
        /// gate. Defaults to 300000 ms (5 minutes).
        #[serde(default = "default_permission_prompt_timeout_ms")]
        pub permission_prompt_timeout_ms: u64 = default_permission_prompt_timeout_ms(),
        /// How long the runtime waits for a consumer to answer an interactive
        /// *user-input* prompt before failing safe (#401).
        ///
        /// Same fail-safe semantics as `permission_prompt_timeout_ms`. Typing
        /// an answer takes longer than clicking approve/deny, so this defaults
        /// higher: 600000 ms (10 minutes).
        #[serde(default = "default_user_input_prompt_timeout_ms")]
        pub user_input_prompt_timeout_ms: u64 = default_user_input_prompt_timeout_ms(),
        /// Interval between health probe pings in milliseconds.
        ///
        /// Set to `0` to disable periodic health checks.
        #[serde(default = "default_health_interval_ms")]
        pub health_interval_ms: u64 = default_health_interval_ms(),
        /// Timeout for the plugin handshake response in milliseconds.
        ///
        /// A plugin that accepts the socket connection but never replies to
        /// the `Handshake` request will fail after this duration instead of
        /// blocking startup indefinitely. Plugins that perform heavy
        /// initialization (model loading, etc.) should respond to the
        /// handshake promptly and defer expensive work until afterwards.
        ///
        /// Unlike `health_interval_ms`, `0` does **not** disable the timeout:
        /// it makes the handshake fail immediately. Use a large value if a
        /// plugin legitimately needs a long time before answering.
        #[serde(default = "default_handshake_timeout_ms")]
        pub handshake_timeout_ms: u64 = default_handshake_timeout_ms(),
        /// Allow insecure MCP HTTP URLs (local development opt-in).
        ///
        /// Defaults to `false` (deny). When `false`, MCP HTTP servers must use
        /// HTTPS and loopback addresses (`127.0.0.0/8`, `::1`) are refused.
        /// Setting this to `true` permits plain-`http://` URLs and loopback
        /// endpoints so a locally-running MCP server can be reached during
        /// development.
        ///
        /// This opt-in never relaxes the link-local block: cloud-metadata
        /// addresses (`169.254.0.0/16`, `fe80::/10`) are always refused.
        pub mcp_allow_insecure_urls: bool = false,
        /// MCP servers to connect to.
        pub mcp_servers: Vec<crate::mcp_config::McpServerConfig> = Vec::new(),
    }
);

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "unit tests use Result::expect for concise assertions"
)]
mod tests {
    /// Smoke-test that the `#[ctor]` registration of the `fs` sandbox schema
    /// does not break schema generation. The full injection into
    /// `ToolConfig.properties.list.properties.fs` requires the complete app
    /// link (verified by `ene-runtime` integration tests); here we only assert
    /// that `generate_schema_json` succeeds with the ctor-registered entry
    /// present in the registry.
    #[test]
    fn fs_sandbox_schema_registration_does_not_break_generation() {
        let schema_json =
            ene_config::generate_schema_json().expect("schema generation should succeed");
        let value: serde_json::Value =
            serde_json::from_str(&schema_json).expect("schema output must be valid JSON");
        assert!(
            value.get("properties").is_some(),
            "settings schema must expose top-level properties"
        );
    }
}
