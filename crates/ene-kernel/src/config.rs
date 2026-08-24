ene_config::define_config!(
    settings,
    "harness",
    /// Agent-loop and context settings.
    pub struct HarnessSettings {
        #[serde(rename = "loop")]
        pub loop_cfg: LoopSettings,
        pub context: ContextSettings,
        pub retry: RetrySettings,
        pub delegation: DelegationSettings,
        /// Soft/hard byte caps for inlining tool results (D-29).
        pub tool_output: ToolOutputSettings,
    }
);

/// Dialogue-lane step budget.
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
    pub token_estimation: TokenEstimation,
}

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            response_reserve_tokens: 4096,
            safety_margin_ratio: 0.1,
            token_estimation: TokenEstimation::Auto,
        }
    }
}

/// Character-based token estimate used when packing a prompt into the window.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case")]
#[schemars(crate = "::ene_config::schemars")]
pub enum TokenEstimation {
    /// CJK-heavy text uses [`Self::Cjk15`], otherwise [`Self::Chars4`].
    #[default]
    Auto,
    /// `ceil(chars / 4)`.
    Chars4,
    /// `ceil(chars * 2 / 3)` (~1.5 chars/token).
    Cjk15,
}

/// Provider-call retry (`harness.retry.*`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct RetrySettings {
    /// Total attempts including the first call.
    pub max_attempts: u32,
    /// Backoff after each failed retryable attempt, in milliseconds.
    pub backoff_ms: Vec<u32>,
}

impl Default for RetrySettings {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_ms: vec![500, 2_000, 8_000],
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

/// Inline vs spill thresholds for surface-tool results.
///
/// Chosen in bytes (not tokens) so a PNG base64 payload and a huge `fs.read`
/// share one cap. Defaults sit at 64 KiB / 256 KiB: large enough for metadata
/// JSON, small enough that a screenshot cannot bloat the conversation log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ToolOutputSettings {
    /// Results larger than this are spilled; the log keeps a summary + ref.
    pub soft_limit_bytes: u64,
    /// Spill summaries shrink further once the body exceeds this size.
    pub hard_limit_bytes: u64,
}

impl Default for ToolOutputSettings {
    fn default() -> Self {
        Self {
            soft_limit_bytes: 64 * 1024,
            hard_limit_bytes: 256 * 1024,
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
    /// Maximum bytes copied from `skills/` into a backup generation.
    pub skills_max_bytes: u64,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            auto: true,
            retention: 7,
            skills_max_bytes: 100 * 1024 * 1024,
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

/// Dialogue-lane slice of `mind.inner.*`. The full `mind` section is
/// `ene_companion::MindSettings`; core projects the inner window here so the
/// kernel does not depend on companion persistence.
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct LaneMindSettings {
    pub inner: InnerSettings,
}

impl LaneMindSettings {
    #[must_use]
    pub fn from_inner_window(
        self_reference_window: u32,
        auto_emotion_events: bool,
        derive_from_thinking: bool,
    ) -> Self {
        Self {
            inner: InnerSettings {
                self_reference_window,
                auto_emotion_events,
                derive_from_thinking,
            },
        }
    }
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
    /// Provider-seam bindings for each task lane.
    pub struct AiSettings {
        pub tasks: AiTasks,
    }
);

/// One `ai.tasks.*` lane.
///
/// Adding a task: append a variant here and a matching named field on
/// [`AiTasks`] - the drift test in this module fails when the two diverge,
/// `ene_fiber::task_seam` must then cover the new variant (exhaustive
/// match), and desktop/stage UI pages follow only if operators need to bind
/// it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AiTaskKind {
    Chat,
    Classifier,
    Embedding,
    Proactive,
    Tts,
    Stt,
    Approve,
    Job,
}

impl AiTaskKind {
    /// Every lane in `AiTasks` field order.
    pub const ALL: &'static [Self] = &[
        Self::Chat,
        Self::Classifier,
        Self::Embedding,
        Self::Proactive,
        Self::Tts,
        Self::Stt,
        Self::Approve,
        Self::Job,
    ];

    /// `ai.tasks.<name>` segment and vault key suffix (`ai.<name>`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Classifier => "classifier",
            Self::Embedding => "embedding",
            Self::Proactive => "proactive",
            Self::Tts => "tts",
            Self::Stt => "stt",
            Self::Approve => "approve",
            Self::Job => "job",
        }
    }

    /// Index into fixed-size per-lane storage sized to `ALL`.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|kind| *kind == self).unwrap_or(0)
    }

    /// Whether an unconfigured binding falls back to the chat lane.
    #[must_use]
    pub fn falls_back_to_chat(self) -> bool {
        matches!(
            self,
            Self::Classifier | Self::Embedding | Self::Proactive | Self::Approve | Self::Job
        )
    }
}

impl std::str::FromStr for AiTaskKind {
    type Err = ();

    fn from_str(task: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.name() == task)
            .ok_or(())
    }
}

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
    pub approve: TaskBinding,
    pub job: TaskBinding,
}

impl AiTasks {
    /// Binding for one lane.
    #[must_use]
    pub fn binding(&self, kind: AiTaskKind) -> &TaskBinding {
        match kind {
            AiTaskKind::Chat => &self.chat,
            AiTaskKind::Classifier => &self.classifier,
            AiTaskKind::Embedding => &self.embedding,
            AiTaskKind::Proactive => &self.proactive,
            AiTaskKind::Tts => &self.tts,
            AiTaskKind::Stt => &self.stt,
            AiTaskKind::Approve => &self.approve,
            AiTaskKind::Job => &self.job,
        }
    }

    /// Mutable binding for one lane.
    pub fn binding_mut(&mut self, kind: AiTaskKind) -> &mut TaskBinding {
        match kind {
            AiTaskKind::Chat => &mut self.chat,
            AiTaskKind::Classifier => &mut self.classifier,
            AiTaskKind::Embedding => &mut self.embedding,
            AiTaskKind::Proactive => &mut self.proactive,
            AiTaskKind::Tts => &mut self.tts,
            AiTaskKind::Stt => &mut self.stt,
            AiTaskKind::Approve => &mut self.approve,
            AiTaskKind::Job => &mut self.job,
        }
    }

    /// Configured binding for the lane, or chat plus the resolved origin lane
    /// when the lane is unconfigured and allowed to fall back. TTS/STT have no
    /// fallback: their unconfigured bindings are returned as-is.
    #[must_use]
    pub fn effective_binding(&self, kind: AiTaskKind) -> (TaskBinding, AiTaskKind) {
        let specific = self.binding(kind);
        if !specific.is_unconfigured() || kind == AiTaskKind::Chat || !kind.falls_back_to_chat() {
            return (specific.clone(), kind);
        }
        (self.chat.clone(), AiTaskKind::Chat)
    }
}

/// One `ai.tasks.*` row. `plugin` is a `provider.*` id when configured.
#[derive(
    Debug, Clone, Default, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct TaskBinding {
    pub plugin: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub base_url: String,
    #[serde(default)]
    pub voice: String,
    /// Absolute engine binary for a managed loopback sidecar (P-1006).
    #[serde(default)]
    pub server_path: String,
    /// Catalog-injected CAS path for the same binary; used when `server_path` is empty.
    #[serde(default)]
    pub cas_path: String,
    /// GGUF / weights path passed to the sidecar as `-m` when `server_args` is empty.
    #[serde(default)]
    pub model_path: String,
    /// Sidecar argv. `{port}` is replaced with the host-assigned loopback port.
    #[serde(default)]
    pub server_args: Vec<String>,
    /// Sidecar health timeout. `None` uses the plugin default.
    #[serde(default)]
    pub startup_timeout_secs: Option<u32>,
    /// Opt-in vision. Unknown or omitted stays false so a text-only model
    /// never receives `LlmImage` payloads.
    #[serde(default)]
    pub supports_images: bool,
    /// Operator cap on the model context window in tokens.
    #[serde(default)]
    pub context_window: Option<u32>,
}

impl TaskBinding {
    #[must_use]
    pub fn echo() -> Self {
        Self {
            plugin: "echo".to_owned(),
            model: "echo".to_owned(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn is_unconfigured(&self) -> bool {
        self.plugin.is_empty() || self.plugin == "echo"
    }

    #[must_use]
    pub fn accepts_images(&self) -> bool {
        !self.is_unconfigured() && self.supports_images
    }
}

ene_config::define_config!(
    settings,
    "plugins",
    /// Launch profile, install home, and IPC limits.
    pub struct PluginSettings {
        pub profile: String = "desktop".to_owned(),
        pub home_dir: String,
        pub policy: PluginPolicySettings,
        pub ipc: PluginIpcSettings,
    }
);

/// `plugins.policy.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct PluginPolicySettings {
    pub approval_mode: String,
    pub allow_unverified: bool,
}

impl Default for PluginPolicySettings {
    fn default() -> Self {
        Self {
            approval_mode: "policy".to_owned(),
            allow_unverified: false,
        }
    }
}

/// `plugins.ipc.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct PluginIpcSettings {
    pub max_frame_bytes: u32,
    pub bulk_threshold_bytes: u32,
}

impl Default for PluginIpcSettings {
    fn default() -> Self {
        Self {
            max_frame_bytes: 1_048_576,
            bulk_threshold_bytes: 65_536,
        }
    }
}

impl PluginSettings {
    #[must_use]
    pub fn kind(&self) -> PluginProfileKind {
        PluginProfileKind::parse(&self.profile)
    }

    #[must_use]
    pub fn resolved_home(&self, data_dir: &std::path::Path) -> std::path::PathBuf {
        if self.home_dir.is_empty() {
            data_dir.join("plugins")
        } else {
            std::path::PathBuf::from(&self.home_dir)
        }
    }
}

/// Shipped launch profiles (P-1002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginProfileKind {
    Desktop,
    Minimal,
    Headless,
}

impl PluginProfileKind {
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "minimal" => Self::Minimal,
            "headless" => Self::Headless,
            _ => Self::Desktop,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Minimal => "minimal",
            Self::Headless => "headless",
        }
    }

    #[must_use]
    pub const fn harness_plugins(self) -> &'static [&'static str] {
        match self {
            Self::Desktop => &[
                "tool.utility",
                "tool.fs",
                "tool.exec",
                "tool.web",
                "tool.app",
            ],
            Self::Minimal => &["tool.utility"],
            Self::Headless => &["tool.utility", "tool.fs", "tool.exec", "tool.web"],
        }
    }

    #[must_use]
    pub const fn includes_mcp(self) -> bool {
        !matches!(self, Self::Minimal)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{AiTaskKind, AiTasks, TaskBinding};

    #[test]
    fn accepts_images_requires_configured_vision_flag() {
        assert!(!TaskBinding::default().accepts_images());
        let text_only = TaskBinding {
            plugin: "provider.openai".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            ..TaskBinding::default()
        };
        assert!(!text_only.is_unconfigured());
        assert!(!text_only.accepts_images());
        let vision = TaskBinding {
            plugin: "provider.openai".to_owned(),
            model: "gpt-4o".to_owned(),
            supports_images: true,
            ..TaskBinding::default()
        };
        assert!(vision.accepts_images());
        let echo_flagged = TaskBinding {
            plugin: "echo".to_owned(),
            supports_images: true,
            ..TaskBinding::default()
        };
        assert!(!echo_flagged.accepts_images());
    }

    #[test]
    fn omitted_supports_images_deserializes_false() {
        let binding: TaskBinding =
            serde_json::from_str(r#"{"plugin":"provider.x","model":"m"}"#).unwrap();
        assert!(!binding.supports_images);
        assert!(!binding.accepts_images());
    }

    #[test]
    fn json_keys_match_task_kinds() {
        let keys: BTreeSet<String> = serde_json::to_value(AiTasks::default())
            .unwrap_or_default()
            .as_object()
            .map(|fields| fields.keys().cloned().collect())
            .unwrap_or_default();
        assert_eq!(keys.len(), AiTaskKind::ALL.len());
        for kind in AiTaskKind::ALL {
            assert!(
                keys.contains(kind.name()),
                "missing tasks field: {}",
                kind.name()
            );
        }
    }

    #[test]
    fn binding_accessor_round_trips() {
        let mut tasks = AiTasks::default();
        for kind in AiTaskKind::ALL {
            tasks.binding_mut(*kind).model = kind.name().to_owned();
        }
        assert_eq!(tasks.chat.model, "chat");
        assert_eq!(tasks.job.model, "job");
        for kind in AiTaskKind::ALL {
            assert_eq!(tasks.binding(*kind).model, kind.name());
        }
        assert_eq!(AiTaskKind::Chat.index(), 0);
        assert_eq!(AiTaskKind::Job.index(), AiTaskKind::ALL.len() - 1);
    }

    #[test]
    fn effective_binding_falls_back_like_the_daemon_contract() {
        let mut tasks = AiTasks::default();
        tasks.classifier.plugin = "provider.x".to_owned();
        tasks.tts.plugin = "provider.tts".to_owned();

        let (binding, origin) = tasks.effective_binding(AiTaskKind::Classifier);
        assert_eq!(origin, AiTaskKind::Classifier);
        assert!(!binding.is_unconfigured());

        // Unconfigured fallback lanes resolve to chat.
        for kind in [
            AiTaskKind::Proactive,
            AiTaskKind::Embedding,
            AiTaskKind::Approve,
            AiTaskKind::Job,
        ] {
            let (binding, origin) = tasks.effective_binding(kind);
            assert_eq!(origin, AiTaskKind::Chat);
            assert!(binding.is_unconfigured());
        }

        // Voice lanes never fall back.
        let (binding, origin) = tasks.effective_binding(AiTaskKind::Tts);
        assert_eq!(origin, AiTaskKind::Tts);
        assert_eq!(binding.plugin, "provider.tts");

        let (_, origin) = tasks.effective_binding(AiTaskKind::Chat);
        assert_eq!(origin, AiTaskKind::Chat);
    }
}
