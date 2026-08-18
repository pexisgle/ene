ene_config::define_config!(
    settings,
    "harness",
    /// Agent-loop and context settings.
    pub struct HarnessSettings {
        #[serde(rename = "loop")]
        pub loop_cfg: LoopSettings,
        pub context: ContextSettings,
        pub delegation: DelegationSettings,
    }
);

/// Dialogue-lane step budget and retry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct LoopSettings {
    /// Surface-lane step cap; reaching it later upgrades to delegation (W5).
    pub max_steps_per_turn: u32,
}

impl Default for LoopSettings {
    fn default() -> Self {
        Self {
            max_steps_per_turn: 4,
        }
    }
}

/// Token-window knobs (values tuned in implementation, D-29).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ContextSettings {
    pub response_reserve_tokens: u32,
    pub safety_margin_ratio: f32,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            response_reserve_tokens: 4096,
            safety_margin_ratio: 0.1,
        }
    }
}

/// Resource guards for public/internal delegations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct DelegationSettings {
    pub max_active: u32,
    pub step_budget: u32,
    pub wall_timeout_secs: u32,
    pub max_depth: u32,
    pub question_timeout_hours: u32,
}

impl Default for DelegationSettings {
    fn default() -> Self {
        Self {
            max_active: 8,
            step_budget: 64,
            wall_timeout_secs: 3_600,
            max_depth: 3,
            question_timeout_hours: 24,
        }
    }
}

ene_config::define_config!(
    settings,
    "core",
    /// Core-daemon process settings.
    pub struct CoreSettings {
        pub data_dir: String,
        pub diag: DiagSettings,
        pub server: ServerSettings,
        pub backup: BackupSettings,
        pub clients: ClientsSettings,
    }
);

/// HTTP/WS bind and token file (relative to the data dir unless absolute).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ServerSettings {
    /// Bind address. Port `0` asks the OS for an ephemeral port.
    pub bind: String,
    /// Token file name or path. Generated at boot when missing.
    pub token_file: String,
    /// Live-bus / WS broadcast capacity.
    pub ws_send_buffer: u32,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:0".to_owned(),
            token_file: "api.token".to_owned(),
            ws_send_buffer: 256,
        }
    }
}

/// Online backup of stores into `<data>/backups/<ts>/`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct BackupSettings {
    pub auto: bool,
    pub retention: u32,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            auto: true,
            retention: 7,
        }
    }
}

/// Multi-client exclusive-resource policy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ClientsSettings {
    /// `explicit` (claim endpoint) or `last_used`.
    pub audio_active_policy: String,
    pub approval_broadcast: bool,
}

impl Default for ClientsSettings {
    fn default() -> Self {
        Self {
            audio_active_policy: "explicit".to_owned(),
            approval_broadcast: true,
        }
    }
}

/// Local span ring (P-517).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct DiagSettings {
    pub enabled: bool,
    pub ring_size: u32,
    pub retention_days: u32,
}

impl Default for DiagSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            ring_size: 100_000,
            retention_days: 3,
        }
    }
}

/// Inner-channel window passed into the lane. Full `mind.*` lives in `ene-companion`.
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct MindSettings {
    pub inner: InnerSettings,
}

/// `mind.inner.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct InnerSettings {
    pub self_reference_window: u32,
    pub auto_emotion_events: bool,
    pub derive_from_thinking: bool,
}

impl Default for InnerSettings {
    fn default() -> Self {
        Self {
            self_reference_window: 24,
            auto_emotion_events: true,
            derive_from_thinking: true,
        }
    }
}

ene_config::define_config!(
    settings,
    "ai",
    /// Provider-seam bindings. `plugin = "echo"` is the offline host model.
    pub struct AiSettings {
        pub tasks: AiTasks,
    }
);

/// Per-task provider binding (`ai.tasks.<name>`).
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct AiTasks {
    pub chat: TaskBinding,
    pub classifier: TaskBinding,
    pub embedding: TaskBinding,
    pub proactive: TaskBinding,
    pub tts: TaskBinding,
    pub stt: TaskBinding,
}

/// One `ai.tasks.*` row. `plugin` is `echo` or a `provider.*` id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct TaskBinding {
    pub plugin: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub base_url: String,
    #[serde(default)]
    pub voice: String,
}

impl Default for TaskBinding {
    fn default() -> Self {
        Self::echo()
    }
}

impl TaskBinding {
    #[must_use]
    pub fn echo() -> Self {
        Self {
            plugin: "echo".to_owned(),
            model: "echo".to_owned(),
            max_tokens: None,
            base_url: String::new(),
            voice: String::new(),
        }
    }

    #[must_use]
    pub fn uses_echo(&self) -> bool {
        self.plugin.is_empty() || self.plugin == "echo"
    }
}
