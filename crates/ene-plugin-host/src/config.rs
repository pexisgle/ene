//! Plugin system configuration section.

use std::collections::HashMap;

use ene_approval::{ApprovalPolicy, PluginApprovalPolicy, SignedManifest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

fn default_max_fds() -> u64 {
    1024
}

const fn default_max_temp_mb() -> u64 {
    1024
}

/// Per-plugin OS sandbox settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct SandboxEntryConfig {
    /// Whether the OS sandbox is applied to this plugin.
    ///
    /// Defaults to `false` until every built-in plugin has been migrated to
    /// the broker channel; enabled plugins fail to start (never degrade)
    /// when a required layer cannot be initialized.
    pub enabled: bool,
    /// Apply the Landlock filesystem allowlist (Linux).
    pub landlock: bool,
    /// Apply the seccomp dangerous-syscall filter (Linux).
    pub seccomp: bool,
    /// Set `no_new_privs` (Linux).
    pub no_new_privs: bool,
    /// Place the plugin in a fresh network namespace (Linux; requires
    /// privileges). When on, the plugin has no direct network at all.
    pub network_namespace: bool,
    /// Apply cgroup v2 memory/pids/cpu limits (Linux; requires a delegated
    /// cgroupfs).
    pub cgroup: bool,
    /// Apply the Windows Job Object (kill-on-close + resource limits).
    pub job_object: bool,
    /// Extra read-only paths the plugin may see (in addition to the host
    /// computed defaults: binary/lib dirs, CA roots, assets, artifacts).
    #[serde(default)]
    pub allowed_read_paths: Vec<String>,
    /// Extra writable paths (in addition to the per-plugin temp dir, the
    /// IPC socket dirs, and write-granted FS slots).
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    /// Maximum open file descriptors (`0` = no rlimit).
    #[serde(default = "default_max_fds")]
    pub max_fds: u64,
    /// Maximum address space in MiB (`0` = no rlimit).
    pub max_memory_mb: u64,
    /// Maximum file size a child may write, in MiB (`0` = no rlimit).
    pub max_file_size_mb: u64,
    /// Per-plugin temp directory cap in MiB.
    #[serde(default = "default_max_temp_mb")]
    pub max_temp_mb: u64,
}

impl Default for SandboxEntryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            landlock: true,
            seccomp: true,
            no_new_privs: true,
            network_namespace: false,
            cgroup: false,
            job_object: true,
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            max_fds: default_max_fds(),
            max_memory_mb: 0,
            max_file_size_mb: 0,
            max_temp_mb: default_max_temp_mb(),
        }
    }
}

/// One user-approved filesystem grant: logical slot → canonical path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FsGrantConfig {
    /// Logical slot name the plugin declared in its manifest.
    pub slot: String,
    /// Real path the user chose for this slot. Stored as configured and
    /// canonicalized at load time.
    pub path: String,
    /// Grant read access.
    pub read: bool,
    /// Grant write access.
    pub write: bool,
}

/// A trusted publisher key for manifest verification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TrustedPublisherConfig {
    /// Publisher id (matches `PluginManifest.publisher`).
    pub publisher: String,
    /// Hex-encoded Ed25519 verifying key.
    pub public_key_hex: String,
}

/// Catalog signing keys for artifact verification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CatalogKeyConfig {
    /// Key id referenced by signed catalogs.
    pub key_id: String,
    /// Hex-encoded Ed25519 verifying key.
    pub public_key_hex: String,
}

/// Signed artifact catalog configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ArtifactConfig {
    /// Whether the artifact system is active. Catalog-dependent plugins
    /// refuse to start when `true` and no catalog is configured.
    pub enabled: bool,
    /// HTTPS URL of the signed catalog metadata.
    pub catalog_url: Option<String>,
    /// Catalog signing keys.
    pub catalog_keys: Vec<CatalogKeyConfig>,
    /// Root directory for the CAS + installation state. Defaults to
    /// `app_data_dir()/artifacts`.
    pub root_dir: Option<String>,
    /// Maximum artifact size in bytes.
    pub max_bytes: u64,
    /// Catalog refresh interval in hours (startup + manual refresh always
    /// happen).
    pub refresh_hours: u64,
    /// Per-hop download timeout in milliseconds.
    pub timeout_ms: u64,
    /// Maximum redirect hops per download.
    pub max_redirects: usize,
}

impl Default for ArtifactConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            catalog_url: None,
            catalog_keys: Vec::new(),
            root_dir: None,
            max_bytes: 8 * 1024 * 1024 * 1024,
            refresh_hours: 6,
            timeout_ms: 60_000,
            max_redirects: 5,
        }
    }
}

/// Web-file download configuration (browsing downloads, not artifacts).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DownloadConfig {
    /// Maximum bytes per download.
    pub max_bytes: u64,
    /// Maximum redirect hops.
    pub max_redirects: usize,
    /// Optional auto-save preset. When set, `WebFileSave=Allow` saves without
    /// a prompt; when unset, even `Allow` still shows the confirmation
    /// (destination, type, size, SHA-256).
    pub auto_save: Option<AutoSaveConfig>,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024 * 1024,
            max_redirects: 5,
            auto_save: None,
        }
    }
}

/// Auto-save preset for browsing downloads.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AutoSaveConfig {
    /// Destination directory.
    pub dir: String,
    /// Maximum bytes accepted by the preset.
    pub max_bytes: u64,
    /// Name-conflict handling (never overwrite automatically).
    pub conflict: ene_plugin_proto::ConflictMode,
}

/// Default plugin list containing the builtin tool and provider plugins.
fn default_plugin_list() -> HashMap<String, PluginEntry> {
    // Pure computation plugins carry no filesystem, network, or process
    // needs, so the OS sandbox is safe to enforce for them from day one.
    // Broker-migrated built-ins (`web`, `fs`, `openai`) are sandboxed too:
    // every OS-touching operation they perform goes through the host
    // (see `docs/concepts/sandbox-and-approvals.md`). Remaining built-ins
    // stay unsandboxed until their broker migration lands.
    let sandboxed_pure = PluginEntry {
        sandbox: Some(SandboxEntryConfig {
            enabled: true,
            ..SandboxEntryConfig::default()
        }),
        ..PluginEntry::default()
    };
    let mut list: HashMap<String, PluginEntry> = [
        "app",
        "browser",
        "calc",
        "calendar",
        "counter",
        "fs",
        "geo",
        "git",
        "homeassistant",
        "openai",
        "random",
        "utility",
        "web",
    ]
    .into_iter()
    .map(|name| (name.to_string(), PluginEntry::default()))
    .collect();

    for name in ["calc", "counter", "random", "web", "fs"] {
        list.insert(name.to_string(), sandboxed_pure.clone());
    }

    // The Anthropic provider plugin authenticates through broker credential
    // injection (the host resolves `ai.providers.<kind>.api_key` and
    // injects it as `x-api-key` at request time), so it runs sandboxed like
    // the other broker-migrated providers.
    list.insert(
        "anthropic".to_string(),
        PluginEntry {
            sandbox: Some(SandboxEntryConfig {
                enabled: true,
                ..SandboxEntryConfig::default()
            }),
            ..PluginEntry::default()
        },
    );

    // The OpenAI-compatible provider plugin authenticates through broker
    // credential injection (the host resolves `ai.providers.<kind>.api_key`
    // and injects it at request time), so only OPENAI_BASE_URL is forwarded
    // for the plugin-side base-URL fallback.
    list.insert(
        "openai".to_string(),
        PluginEntry {
            sandbox: Some(SandboxEntryConfig {
                enabled: true,
                ..SandboxEntryConfig::default()
            }),
            env_passthrough: vec!["OPENAI_BASE_URL".to_string()],
            ..PluginEntry::default()
        },
    );

    // The OpenAI Speech API TTS provider plugin authenticates with the same
    // OPENAI_API_KEY credential and honors the same base URL override as the
    // openai plugin.
    list.insert(
        "openai-tts".to_string(),
        PluginEntry {
            env_passthrough: vec!["OPENAI_API_KEY".to_string(), "OPENAI_BASE_URL".to_string()],
            ..PluginEntry::default()
        },
    );

    // The local GGUF provider plugin needs no host environment and loads no
    // model until one is configured, so the default entry is a plain enabled
    // process.
    list.insert("llama-cpp".to_string(), PluginEntry::default());

    // The llama-server sidecar provider plugin (experimental successor to the
    // in-process llama-cpp backend) stays disabled by default: it is inert
    // until the sidecar binary is installed and the entry is enabled, so
    // shipping it costs nothing but keeps the switch-over path ready.
    list.insert(
        "llama-server".to_string(),
        PluginEntry {
            enable: false,
            ..PluginEntry::default()
        },
    );

    // The VOICEVOX-compatible TTS provider plugin talks to a local engine
    // over plain HTTP (no credentials); it is inert until
    // `ai.tts.provider = "voicevox"` selects it.
    list.insert("voicevox".to_string(), PluginEntry::default());

    // The local Kokoro-TTS provider plugin loads the ONNX model on first use
    // (no host environment or credentials); it is inert until
    // `ai.tts.provider = "kokoro"` selects it.
    list.insert("kokoro".to_string(), PluginEntry::default());

    // The local ONNX provider plugin (Silero VAD + onnx-runner / g2p
    // capabilities) loads the ONNX model on first use; it is inert until
    // `ai.vad.provider = "silero"` selects it.
    list.insert("onnx".to_string(), PluginEntry::default());

    // The local whisper.cpp STT provider plugin loads the GGUF model on
    // first use; it is inert until `ai.stt.provider = "whisper"` selects it.
    list.insert("whisper".to_string(), PluginEntry::default());

    // The Edge-TTS provider plugin talks to Microsoft's free, keyless Edge
    // Read Aloud WebSocket endpoint; it is inert until
    // `ai.tts.provider = "edge-tts"` selects it.
    list.insert("edge-tts".to_string(), PluginEntry::default());

    // The ElevenLabs TTS provider plugin needs ELEVENLABS_API_KEY forwarded
    // from the host environment; without it the provider cannot authenticate.
    list.insert(
        "elevenlabs".to_string(),
        PluginEntry {
            env_passthrough: vec![
                "ELEVENLABS_API_KEY".to_string(),
                "ELEVENLABS_BASE_URL".to_string(),
            ],
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
    /// Signed manifest for this plugin. Built-in plugins fall back to the
    /// host's embedded manifest when this is absent.
    #[serde(default)]
    pub manifest: Option<SignedManifest>,
    /// OS sandbox settings; `None` inherits the global default.
    #[serde(default)]
    pub sandbox: Option<SandboxEntryConfig>,
    /// User-approved filesystem grants (logical slot → real path).
    #[serde(default)]
    pub fs_grants: Vec<FsGrantConfig>,
    /// Host-owned credentials served through the `Credential` broker. Never
    /// delivered inside the plugin config blob.
    #[serde(default)]
    pub credentials: std::collections::BTreeMap<String, String>,
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
            manifest: None,
            sandbox: None,
            fs_grants: Vec::new(),
            credentials: std::collections::BTreeMap::new(),
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

/// Admission budget override for one [`ResourceClass`](ene_plugin_proto::ResourceClass).
///
/// The `class` value uses the same externally tagged JSON form as the wire
/// (`"Cpu"` / `{"Gpu":{"device":0}}` / `"Network"`), so one vocabulary covers
/// both the plugin declaration and the host configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct ResourceClassBudget {
    /// The class this entry budgets. `Gpu` classes are gated by default
    /// (one concurrent job per device); `Cpu` / `Network` are only gated
    /// when an entry names them.
    pub class: ene_plugin_proto::ResourceClass,
    /// Maximum concurrent in-flight jobs for this class. `None` uses the
    /// class default: 1 for GPU devices, the logical CPU count for `Cpu`,
    /// 4 for `Network`. The value is clamped to at least 1: a zero-permit
    /// class would deadlock every request against it, which is a worse
    /// failure mode than the clamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permits: Option<usize>,
    /// How many additional callers may wait for a permit before requests
    /// fail fast with `Busy`. `None` uses the default of 8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_depth: Option<usize>,
}

impl Default for ResourceClassBudget {
    fn default() -> Self {
        Self {
            class: ene_plugin_proto::ResourceClass::Cpu,
            permits: None,
            queue_depth: None,
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
        /// Per-`ResourceClass` admission budgets for provider requests.
        ///
        /// Every plugin that declares the same class (e.g. two local LLM
        /// plugins offloading to GPU device 0) shares the class's budget, so
        /// the host never sends more concurrent GPU-bound requests than the
        /// device can serve. `Gpu` classes are gated even without an entry
        /// (one job per device, up to 8 callers waiting); add an entry to
        /// raise the concurrency or widen the wait queue. `Cpu` and
        /// `Network` classes are not gated unless an entry names them, so
        /// cloud providers keep their declared per-plugin concurrency
        /// untouched. Permits are held for the duration of a request (a
        /// stream, or a single completion) and released automatically when
        /// the request ends or the serving plugin crashes.
        pub resource_classes: Vec<ResourceClassBudget> = Vec::new(),
        /// Global approval policy (per-category modes; defaults to `Ask`).
        pub approval: ApprovalPolicy = ApprovalPolicy::default(),
        /// Per-plugin approval overrides (`Inherit` delegates to the global
        /// policy).
        pub plugin_approval: std::collections::BTreeMap<String, PluginApprovalPolicy> =
            std::collections::BTreeMap::new(),
        /// Trusted publisher keys for third-party manifest verification.
        pub trusted_publishers: Vec<TrustedPublisherConfig> = Vec::new(),
        /// Audit log path for approval decisions. Defaults to
        /// `app_data_dir()/audit/plugin-approval.jsonl`.
        pub audit_log_path: Option<String> = None,
        /// Default OS-sandbox settings for plugins without a per-entry
        /// `sandbox` override.
        pub sandbox: SandboxEntryConfig = SandboxEntryConfig::default(),
        /// Signed artifact catalog / CAS configuration.
        pub artifact: ArtifactConfig = ArtifactConfig::default(),
        /// Web-file download limits and auto-save preset.
        pub download: DownloadConfig = DownloadConfig::default(),
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

    /// `plugins.resource_classes` must round-trip through JSON with the same
    /// externally tagged class form the wire uses, and default to empty when
    /// absent.
    #[test]
    fn resource_classes_config_round_trips() {
        let json = serde_json::json!({
            "resource_classes": [
                { "class": { "Gpu": { "device": 0 } }, "permits": 2, "queue_depth": 4 },
                { "class": "Cpu", "permits": 8 }
            ]
        });
        let config: PluginConfig = serde_json::from_value(json.clone()).expect("parses");
        assert_eq!(config.resource_classes.len(), 2);
        let gpu = &config.resource_classes[0];
        assert_eq!(
            gpu.class,
            ene_plugin_proto::ResourceClass::Gpu { device: 0 }
        );
        assert_eq!(gpu.permits, Some(2));
        assert_eq!(gpu.queue_depth, Some(4));
        let cpu = &config.resource_classes[1];
        assert_eq!(cpu.class, ene_plugin_proto::ResourceClass::Cpu);
        assert_eq!(cpu.permits, Some(8));
        assert_eq!(cpu.queue_depth, None);

        let back = serde_json::to_value(&config).expect("serializes");
        assert_eq!(back.get("resource_classes"), json.get("resource_classes"));

        let empty: PluginConfig =
            serde_json::from_value(serde_json::json!({})).expect("defaults apply");
        assert!(empty.resource_classes.is_empty());
    }
}
