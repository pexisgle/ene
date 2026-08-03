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

const fn default_parallel_tool_calls_max() -> usize {
    4
}

/// Default per-plugin DB storage quota, in MiB.
///
/// `256` is deliberately generous: every built-in stateful plugin (`fs`,
/// `utility`, `browser`) stores kilobytes-to-low-megabytes of bookkeeping, so
/// the cap never constrains legitimate first-party use, while still bounding
/// how much of the *shared* `memory.db` a single runaway or malicious plugin
/// can claim before its writes are refused. It is low enough that a plugin
/// stuck in a logging loop trips the quota (and surfaces a diagnostic) long
/// before it can exhaust the disk or bloat the database enough to degrade the
/// memory system's queries, backups, and integrity checks. The field is an
/// `Option` so `null` can disable enforcement for a plugin that legitimately
/// needs unbounded storage.
#[expect(
    clippy::unnecessary_wraps,
    reason = "serde default function must return the field type Option<u64>; \
              the Some(256) default is the documented enforcement default"
)]
const fn default_db_quota_mb() -> Option<u64> {
    Some(256)
}

/// Default plugin list containing the builtin tool and provider plugins.
fn default_plugin_list() -> HashMap<String, PluginEntry> {
    let mut list: HashMap<String, PluginEntry> = [
        "app", "browser", "calc", "calendar", "fs", "geo", "random", "utility", "web",
    ]
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

    // The OpenAI-compatible provider plugin needs OPENAI_API_KEY and
    // OPENAI_BASE_URL forwarded from the host environment, mirroring the
    // anthropic entry.
    list.insert(
        "openai".to_string(),
        PluginEntry {
            env_passthrough: vec!["OPENAI_API_KEY".to_string(), "OPENAI_BASE_URL".to_string()],
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
    /// is implemented.
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    /// Per-plugin cap on how much of the shared `memory.db` this plugin's
    /// tables may occupy, in mebibytes.
    ///
    /// Writes (`Insert`/`Upsert`) that would push the plugin's measured
    /// footprint past this cap are rejected with a `QuotaExceeded` error;
    /// reads and deletes remain permitted so the plugin can free space.
    /// Defaults to `Some(256)` — generous enough that no built-in plugin
    /// comes close, while still bounding a runaway plugin
    /// before it can exhaust the disk or bloat the database. Set to `null`
    /// (or omit and rely on the default) — `None` disables enforcement for
    /// a plugin that legitimately needs unbounded storage.
    #[serde(default = "default_db_quota_mb")]
    pub db_quota_mb: Option<u64>,
    /// Plugin-specific configuration (opaque JSON delivered to the plugin at
    /// handshake time and via live `SetConfig` IPC through
    /// `ConfigurablePlugin::set_config`).
    ///
    /// The host does **not** interpret this blob: it is stored and
    /// delivered verbatim. Plugin-owned settings live here (e.g.
    /// `plugins.list.llama-cpp.config.mmproj_url`). The environment override
    /// path is the single-key form
    /// `ENE_PLUGINS__LIST__<NAME>__CONFIG__<KEY>`
    /// (e.g. `ENE_PLUGINS__LIST__ANTHROPIC__CONFIG__API_KEY`): figment's env
    /// provider parses values with TOML-like syntax, so a full JSON object in
    /// `ENE_PLUGINS__LIST__<NAME>__CONFIG` is not reliably supported — set
    /// individual keys instead.
    ///
    /// Do **not** put host-reserved entry keys (`enable`, `checksum`) inside
    /// this object — they collide with [`PluginEntry`] fields and confuse
    /// authors. The host warns when they appear in the delivered blob.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Per-profile plugin configuration (opaque JSON), keyed by profile name.
    ///
    /// One plugin can need different settings per model/profile (e.g.
    /// `plugins.list.kokoro.profiles.<profile>.voices_path`); profile
    /// *selection* is plugin-owned. The whole map is delivered to the plugin
    /// at handshake time via `ConfigurablePlugin::set_profiles`.
    #[serde(default)]
    pub profiles: HashMap<String, serde_json::Value>,
    /// Unknown entry-level keys (anything beyond the declared fields),
    /// preserved verbatim across load → save so the host never drops keys
    /// it does not understand. At plugin startup these flat keys are folded
    /// into the delivered config blob (explicit `config` keys win) so legacy
    /// entries keep working.
    #[serde(flatten)]
    #[schemars(skip)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for PluginEntry {
    fn default() -> Self {
        Self {
            enable: true,
            checksum: None,
            env_passthrough: Vec::new(),
            db_quota_mb: default_db_quota_mb(),
            config: serde_json::Value::Object(serde_json::Map::default()),
            profiles: HashMap::new(),
            extra: serde_json::Map::default(),
        }
    }
}

/// Host-owned [`PluginEntry`] field names that must not appear inside the
/// plugin-delivered `config` object (they collide with typed entry fields).
const HOST_RESERVED_CONFIG_KEYS: &[&str] = &["enable", "checksum"];

impl PluginEntry {
    /// Builds the config blob delivered to the plugin, folding legacy flat
    /// entry-level keys into the nested `config` object (explicit `config`
    /// keys win). Returns `None` when the resulting blob is empty/null.
    ///
    /// Warns when the delivered object contains host-reserved keys
    /// (`enable`, `checksum`) that would confuse plugin authors.
    pub fn delivered_config(&self, plugin_name: &str) -> Option<serde_json::Value> {
        let config = if self.extra.is_empty() {
            Some(self.config.clone())
                .filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
        } else {
            // Legacy flat entry-level keys (from the pre-hierarchy
            // `#[serde(flatten)]` entries) are folded into the delivered
            // config blob so existing settings.json files keep working.
            // Explicit `config` keys win. The fold is in-memory only.
            let mut folded: serde_json::Map<String, serde_json::Value> =
                match self.config.as_object() {
                    Some(obj) => obj.clone(),
                    None => serde_json::Map::new(),
                };
            let mut folded_count = 0_usize;
            for (key, value) in &self.extra {
                if folded.contains_key(key) {
                    continue;
                }
                folded.insert(key.clone(), value.clone());
                folded_count += 1;
            }
            if folded_count > 0 {
                tracing::warn!(
                    component = "PluginEntry",
                    plugin = %plugin_name,
                    count = folded_count,
                    "Folding legacy flat config key(s) into the config blob"
                );
            }
            Some(serde_json::Value::Object(folded))
        };
        if let Some(ref blob) = config {
            warn_reserved_config_keys(plugin_name, blob);
        }
        config
    }

    /// Builds the profiles blob delivered to the plugin, or `None` when empty.
    pub fn delivered_profiles(&self) -> Option<serde_json::Value> {
        (!self.profiles.is_empty())
            .then(|| serde_json::Value::Object(self.profiles.clone().into_iter().collect()))
    }
}

/// Emits a warning when a plugin-delivered config object contains keys that
/// collide with host-owned [`PluginEntry`] fields.
fn warn_reserved_config_keys(plugin_name: &str, config: &serde_json::Value) {
    let Some(obj) = config.as_object() else {
        return;
    };
    for key in HOST_RESERVED_CONFIG_KEYS {
        if obj.contains_key(*key) {
            tracing::warn!(
                component = "PluginEntry",
                plugin = %plugin_name,
                key = %key,
                "plugin config blob contains a host-reserved key; the name is \
                 also a plugins.list.<name> entry field and will confuse \
                 authors — nest plugin-owned settings under distinct keys"
            );
        }
    }
}

// Register the `fs` sandbox tool schema from the host crate: the proto crate
// is wire-ABI only and must not depend on `ene-config`, so the host crate
// (which links both) takes over the registration.
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
        /// *permission* prompt before failing safe.
        ///
        /// The wait is bounded by this timeout **and** selected against the
        /// turn's cancel token, so a consumer that never responds (a lost
        /// event, a headless/automation consumer, a closed window) cannot
        /// hold the turn open forever: on expiry the prompt is treated as
        /// denied and the turn still reaches `Terminal`, releasing the turn
        /// gate. Defaults to 300000 ms (5 minutes).
        ///
        /// Unlike `health_interval_ms`, `0` does **not** disable the wait: it
        /// makes the prompt time out immediately (fail-safe denied unless the
        /// consumer has already answered). Use a large value if a consumer
        /// legitimately needs a long time to answer.
        #[serde(default = "default_permission_prompt_timeout_ms")]
        pub permission_prompt_timeout_ms: u64 = default_permission_prompt_timeout_ms(),
        /// How long the runtime waits for a consumer to answer an interactive
        /// *user-input* prompt before failing safe.
        ///
        /// Same fail-safe semantics as `permission_prompt_timeout_ms`. Typing
        /// an answer takes longer than clicking approve/deny, so this defaults
        /// higher: 600000 ms (10 minutes).
        ///
        /// Unlike `health_interval_ms`, `0` does **not** disable the wait: it
        /// makes the prompt time out immediately (fail-safe cancelled unless
        /// the consumer has already answered). Use a large value if a consumer
        /// legitimately needs a long time to answer.
        #[serde(default = "default_user_input_prompt_timeout_ms")]
        pub user_input_prompt_timeout_ms: u64 = default_user_input_prompt_timeout_ms(),
        /// Maximum number of side-effect-free tool calls executed concurrently
        /// in a single round.
        ///
        /// When the LLM returns multiple tool calls, those that declare
        /// themselves read-only (`SideEffects::ReadOnly`) are run in a bounded
        /// batch of at most this many at once; side-effectful tools always run
        /// sequentially. `0` disables parallelism entirely (every call runs
        /// sequentially). The bound caps simultaneous IPC load on plugin
        /// processes.
        #[serde(default = "default_parallel_tool_calls_max")]
        pub parallel_tool_calls_max: usize = default_parallel_tool_calls_max(),
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
    use super::*;

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

    /// Round-trip: the nested `config` / `profiles` blobs and unknown
    /// entry-level keys must survive serialize → deserialize verbatim, so the
    /// host never drops plugin-owned settings it does not understand.
    #[test]
    fn plugin_entry_round_trips_config_profiles_and_unknown_keys() {
        let json = serde_json::json!({
            "enable": true,
            "checksum": "abc123",
            "env_passthrough": ["ANTHROPIC_API_KEY"],
            "db_quota_mb": 512,
            "config": {
                "mmproj_url": "https://cdn.example/mmproj.gguf",
                "api_key": {"source": "env", "env": "ANTHROPIC_API_KEY"},
                "future_field": {"nested": [1, 2, 3]}
            },
            "profiles": {
                "kokoro": {"voices_path": "/data/voices.bin"}
            },
            "future_entry_level_key": "preserved"
        });
        let entry: PluginEntry =
            serde_json::from_value(json.clone()).expect("deserialize plugin entry");
        assert!(entry.enable);
        assert_eq!(entry.checksum.as_deref(), Some("abc123"));
        assert_eq!(
            entry
                .config
                .get("mmproj_url")
                .and_then(serde_json::Value::as_str),
            Some("https://cdn.example/mmproj.gguf")
        );
        assert_eq!(
            entry.profiles.get("kokoro"),
            Some(&serde_json::json!({"voices_path": "/data/voices.bin"}))
        );
        // Unknown entry-level key lands in the flattened catch-all.
        assert_eq!(
            entry
                .extra
                .get("future_entry_level_key")
                .and_then(serde_json::Value::as_str),
            Some("preserved")
        );

        let back = serde_json::to_value(&entry).expect("serialize plugin entry");
        assert_eq!(
            back.get("config").expect("config field"),
            &serde_json::json!({
                "mmproj_url": "https://cdn.example/mmproj.gguf",
                "api_key": {"source": "env", "env": "ANTHROPIC_API_KEY"},
                "future_field": {"nested": [1, 2, 3]}
            })
        );
        assert_eq!(
            back.get("profiles").expect("profiles field"),
            &serde_json::json!({"kokoro": {"voices_path": "/data/voices.bin"}})
        );
        assert_eq!(
            back.get("future_entry_level_key"),
            Some(&serde_json::json!("preserved"))
        );
    }

    /// The default entry carries an empty `config` object and no profiles, so
    /// the host's `Some(...).filter(non-empty)` handshake gating skips it.
    #[test]
    fn plugin_entry_default_is_empty() {
        let entry = PluginEntry::default();
        assert!(
            entry
                .config
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );
        assert!(entry.profiles.is_empty());
        assert!(entry.extra.is_empty());
    }

    #[test]
    fn delivered_config_passes_reserved_keys_through() {
        // Reserved host keys inside the nested config blob must survive
        // delivery verbatim: `delivered_config` warns via
        // `warn_reserved_config_keys` but never strips the keys.
        let entry = PluginEntry {
            config: serde_json::json!({
                "enable": false,
                "checksum": "deadbeef",
                "api_key": "sk-test"
            }),
            ..PluginEntry::default()
        };
        let blob = entry
            .delivered_config("demo")
            .expect("non-empty config must be delivered");
        assert_eq!(blob.get("enable"), Some(&serde_json::json!(false)));
        assert_eq!(blob.get("checksum"), Some(&serde_json::json!("deadbeef")));
        assert_eq!(blob.get("api_key"), Some(&serde_json::json!("sk-test")));
    }
}
