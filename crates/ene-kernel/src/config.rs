ene_config::define_config!(
    settings,
    "harness",
    /// Agent-loop and context settings.
    pub struct HarnessSettings {
        #[serde(rename = "loop")]
        pub loop_cfg: LoopSettings,
        pub context: ContextSettings,
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

ene_config::define_config!(
    settings,
    "mind",
    /// Inner-channel projection window.
    pub struct MindSettings {
        pub inner: InnerSettings,
    }
);

/// `mind.inner.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct InnerSettings {
    pub self_reference_window: u32,
}

impl Default for InnerSettings {
    fn default() -> Self {
        Self {
            self_reference_window: 8,
        }
    }
}
