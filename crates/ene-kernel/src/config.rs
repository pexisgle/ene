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
    }
);

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
