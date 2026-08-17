ene_config::define_config!(
    settings,
    "store",
    /// Persistence settings for the harness session log.
    pub struct StoreSettings {
        pub sessions: SessionsSettings,
    }
);

/// Conversation-log file and export defaults (`store.sessions.*`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct SessionsSettings {
    /// Empty means `<data>/sessions.db`.
    pub db_path: String,
    /// `NORMAL` (default) or `FULL`.
    pub synchronous: String,
    pub export: ExportSettings,
}

impl Default for SessionsSettings {
    fn default() -> Self {
        Self {
            db_path: String::new(),
            synchronous: "NORMAL".to_owned(),
            export: ExportSettings::default(),
        }
    }
}

/// Export redaction defaults.
#[derive(
    Debug, Default, Clone, serde::Serialize, serde::Deserialize, ene_config::schemars::JsonSchema,
)]
#[serde(crate = "::ene_config::serde", rename_all = "snake_case", default)]
#[schemars(crate = "::ene_config::schemars")]
pub struct ExportSettings {
    pub include_inner: bool,
    pub include_thinking: bool,
}
