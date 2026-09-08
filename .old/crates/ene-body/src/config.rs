ene_config::define_config!(
    settings,
    "body",
    /// Stage render and autonomy knobs.
    pub struct BodySettings {
        pub render: RenderSettings,
        pub autonomy: AutonomySettings,
        pub fallback: FallbackSettings,
    }
);

/// `body.render.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct RenderSettings {
    pub enabled: bool,
    pub max_concurrent: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent: 2,
        }
    }
}

/// `body.autonomy.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct AutonomySettings {
    pub enabled: bool,
}

impl Default for AutonomySettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `body.fallback.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct FallbackSettings {
    pub nearest_expression: bool,
}

impl Default for FallbackSettings {
    fn default() -> Self {
        Self {
            nearest_expression: true,
        }
    }
}

ene_config::define_config!(
    settings,
    "voice",
    /// Duplex voice pipeline owned by the core daemon.
    pub struct VoiceSettings {
        pub enabled: bool = true,
        pub barge_in: BargeInSettings,
        pub keep_raw_audio: bool = false,
        pub input: VoiceInputSettings,
        pub mask_pad_ms: u64 = 200,
    }
);

/// `voice.barge_in.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct BargeInSettings {
    pub enabled: bool,
    pub min_speech_ms: u64,
    pub debounce_ms: u64,
}

impl Default for BargeInSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            min_speech_ms: 400,
            debounce_ms: 300,
        }
    }
}

/// `voice.input.*`
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct VoiceInputSettings {
    pub routing: String,
}

impl Default for VoiceInputSettings {
    fn default() -> Self {
        Self {
            routing: "active_body".to_owned(),
        }
    }
}
