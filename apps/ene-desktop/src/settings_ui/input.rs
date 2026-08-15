//! Per-render scratch buffer for editable text fields.
//!
//! `SettingsInputState` is owned by the [`SettingsUi`](super::SettingsUi) and
//! `sync_from_settings` is called whenever the settings window
//! transitions from hidden → visible.
use std::collections::BTreeMap;

use crate::settings::CharacterSettings;

/// Dynamic `ListConfigOptions` results: dotted field path → (label, value)
/// choices.
pub type PluginOptionsMap = std::collections::BTreeMap<String, Vec<(String, String)>>;

/// Asynchronously fetched UI data with loading / error / retry state.
///
/// The owning page calls [`AsyncData::start`] with a receiver produced by
/// [`crate::ai_bridge::AiBridge::spawn_fetch`] and polls [`AsyncData::poll`]
/// every frame; the render loop never blocks on the underlying actor or IPC
/// round-trips.
#[derive(Debug)]
pub struct AsyncData<T> {
    receiver: Option<tokio::sync::oneshot::Receiver<T>>,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> AsyncData<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            receiver: None,
            data: None,
            error: None,
        }
    }

    /// A request is only accepted when nothing is already in flight or cached.
    pub fn start(&mut self, receiver: tokio::sync::oneshot::Receiver<T>) {
        if self.receiver.is_none() && self.data.is_none() {
            self.receiver = Some(receiver);
            self.error = None;
        }
    }

    /// Used by live-updating views (artifact progress) that must render the
    /// most recent snapshot while the next fetch is in flight.
    pub fn refresh(&mut self, receiver: tokio::sync::oneshot::Receiver<T>) {
        if self.receiver.is_none() {
            self.receiver = Some(receiver);
            self.error = None;
        }
    }

    /// Discards any cached value or error; used by re-validate / reload
    /// buttons.
    pub fn restart(&mut self, receiver: tokio::sync::oneshot::Receiver<T>) {
        self.receiver = Some(receiver);
        self.data = None;
        self.error = None;
    }

    pub fn poll(&mut self) {
        let Some(receiver) = &mut self.receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(value) => {
                self.data = Some(value);
                self.receiver = None;
                self.error = None;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.error = Some("fetch cancelled".to_string());
                self.receiver = None;
            }
        }
    }

    #[must_use]
    pub const fn loading(&self) -> bool {
        self.receiver.is_some()
    }

    /// Used for first-render lazy loading.
    #[must_use]
    pub const fn started(&self) -> bool {
        self.receiver.is_some() || self.data.is_some() || self.error.is_some()
    }
}

impl<T> Default for AsyncData<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default)]
pub struct SettingsInputState {
    pub look_at_strength: String,
    pub model_scale: String,
    pub character_pos_x: String,
    pub character_pos_y: String,
    pub character_pos_z: String,
    pub ai_user_name: String,
    pub ai_chat_provider: String,
    pub ai_chat_model: String,
    pub ai_base_url: String,
    pub ai_api_key_source: String,
    pub ai_api_key: String,
    pub ai_api_key_env: String,
    pub ai_embedding_provider: String,
    pub ai_embedding_model: String,
    pub ai_embedding_dimensions: String,
    pub ai_validation_message: Option<String>,
    pub tts_provider: String,
    pub stt_provider: String,
    /// Enumerated input device names for the microphone picker. Filled by
    /// the Voice page (and the Features audio section) when the window is
    /// shown; empty without the `voice` feature.
    pub mic_devices: Vec<String>,
    /// Cached `/models` catalog per provider key, fetched on demand by the
    /// AI page's refresh button.
    pub model_catalog: BTreeMap<String, Vec<String>>,
    pub model_fetch_error: Option<String>,
    /// Asynchronously cached plugin settings snapshots for the plugin
    /// center page (and Overview / search), refreshed on window open and by
    /// the page's refresh button.
    pub plugin_snapshots: AsyncData<Vec<ene_plugin_host::PluginSettingsSnapshot>>,
    /// Asynchronously cached host-side artifact snapshot for the Engines
    /// page (installed sidecars/models plus catalog targets).
    pub artifact_snapshot: AsyncData<Vec<ene_plugin_host::ArtifactSnapshot>>,
    /// Keyed by artifact id.
    pub artifact_installs: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Receiver<Result<ene_plugin_host::InstalledArtifactView, String>>,
    >,
    /// Keyed by artifact id.
    pub artifact_rollbacks: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Receiver<Result<ene_plugin_host::InstalledArtifactView, String>>,
    >,
    pub artifact_uninstalls:
        std::collections::HashMap<String, tokio::sync::oneshot::Receiver<Result<(), String>>>,
    pub artifact_cancels:
        std::collections::HashMap<String, tokio::sync::oneshot::Receiver<Result<(), String>>>,
    /// Polled every frame while an install is running.
    pub artifact_progress:
        AsyncData<std::collections::BTreeMap<String, Option<ene_plugin_host::ArtifactProgress>>>,
    /// Last error per artifact operation (install/rollback/uninstall),
    /// displayed until the next action or snapshot refresh.
    pub artifact_errors: std::collections::BTreeMap<String, String>,
    /// Two-step confirmation arms for destructive artifact actions
    /// (`artifact_id|action` → armed).
    pub artifact_arm: std::collections::BTreeMap<String, bool>,
    pub catalog_refresh: Option<tokio::sync::oneshot::Receiver<Result<u64, String>>>,
    /// Two-step delete arms for model files on the Engines page
    /// (`plugin|model` → armed).
    pub model_delete_arm: std::collections::HashMap<String, bool>,
    pub schedules: AsyncData<Vec<ene_core::Schedule>>,
    /// Asynchronously cached run history for the currently selected
    /// schedule.
    pub schedule_runs: AsyncData<Vec<ene_core::ScheduleRun>>,
    /// Recent run history of every schedule, for the pending-confirmations
    /// list.
    pub pending_runs: AsyncData<Vec<(i64, Vec<ene_core::ScheduleRun>)>>,
    /// In-flight schedule mutations (click-driven, never per frame).
    pub schedule_toggle_rx:
        Option<tokio::sync::oneshot::Receiver<Result<bool, ene_runtime::EneRuntimeError>>>,
    pub schedule_delete_rx:
        Option<tokio::sync::oneshot::Receiver<Result<bool, ene_runtime::EneRuntimeError>>>,
    pub schedule_add_rx: Option<
        tokio::sync::oneshot::Receiver<Result<ene_core::Schedule, ene_runtime::EneRuntimeError>>,
    >,
    pub schedule_confirm_rx:
        Option<tokio::sync::oneshot::Receiver<Result<bool, ene_runtime::EneRuntimeError>>>,
    /// Schedule currently being edited by the form (`None` = add mode).
    pub schedule_editing: Option<i64>,
    pub schedule_update_rx:
        Option<tokio::sync::oneshot::Receiver<Result<ene_core::Schedule, String>>>,
    /// Detected-but-unconfigured plugin binaries.
    pub discovered_plugins: AsyncData<Vec<String>>,
    /// Dynamic `ListConfigOptions` results per plugin, keyed by the field's
    /// dotted config path.
    pub plugin_options: std::collections::BTreeMap<String, AsyncData<PluginOptionsMap>>,
    pub plugin_validation: std::collections::BTreeMap<String, AsyncData<Vec<String>>>,
    /// Standing permission grants (Permission Center page).
    pub permissions: AsyncData<Vec<ene_runtime::PermissionScope>>,
    /// Message from the last permission mutation (revoke / reset).
    pub permission_action: AsyncData<String>,
    pub connectors: AsyncData<Vec<ene_connector::ConnectorSummary>>,
    /// Status + grants of the selected connector.
    pub connector_detail: AsyncData<(
        Option<ene_connector::ConnectorStatus>,
        Vec<ene_connector::PermissionGrant>,
    )>,
    pub connector_check: AsyncData<Result<ene_connector::HealthStatus, String>>,
    pub sessions: AsyncData<Vec<ene_runtime::PublicSessionMeta>>,
    pub session_search: AsyncData<Vec<(String, ene_runtime::PublicExportedMessage)>>,
    pub session_message: AsyncData<String>,
    pub provider_catalog: AsyncData<Option<ene_runtime::ProviderCatalog>>,
    pub model_list: AsyncData<Vec<String>>,
    pub api_test: AsyncData<Result<(), String>>,
    /// Direct tool-call test results per plugin (e.g. Home Assistant
    /// connectivity).
    pub plugin_tool_test: std::collections::BTreeMap<String, AsyncData<String>>,
    /// In-flight memory-ledger mutation messages (polled each frame).
    pub ledger_pending: Vec<tokio::sync::oneshot::Receiver<String>>,
    pub mcp_statuses: AsyncData<Vec<ene_plugin_host::McpServerStatus>>,
}

impl SettingsInputState {
    pub fn new() -> Self {
        Self {
            ai_embedding_provider: "llama-cpp".to_string(),
            ai_embedding_model: "text-embedding-3-small".to_string(),
            ai_embedding_dimensions: "1536".to_string(),
            ai_api_key_source: "env".to_string(),
            ai_api_key_env: "OPENAI_API_KEY".to_string(),
            ..Self::default()
        }
    }

    pub fn sync_from_settings(
        &mut self,
        settings: &CharacterSettings,
        _ui_state: &crate::settings::UiState,
    ) {
        self.look_at_strength = format!("{:.2}", settings.character_state.look_at_strength);
        self.model_scale = format!("{:.2}", settings.character_state.model_scale);
        self.character_pos_x = format!("{:+.2}", settings.character_state.character_position.x);
        self.character_pos_y = format!("{:+.2}", settings.character_state.character_position.y);
        self.character_pos_z = format!("{:+.2}", settings.character_state.character_position.z);
        self.ai_user_name.clone_from(&settings.config().user_name);
        let ai_cfg = settings.config_section::<ene_runtime::AiConfig>();
        self.ai_chat_provider
            .clone_from(&ai_cfg.tasks.chat.provider);
        if ene_ai::AiConfig::is_local_provider(&ai_cfg.tasks.chat.provider) {
            self.ai_chat_provider.clone_from(&ai_cfg.local_engine);
        }
        self.ai_chat_model = ai_cfg.tasks.chat.model.clone().unwrap_or_default();
        if let Some(def) = ai_cfg.providers.get(&ai_cfg.tasks.chat.provider) {
            self.ai_base_url.clone_from(&def.base_url);
            self.ai_api_key_source.clone_from(&def.api_key.source);
            // Secrets never round-trip back into UI text buffers; the draft
            // tracks them by state instead.
            self.ai_api_key.clear();
            self.ai_api_key_env.clone_from(&def.api_key.env);
        } else {
            self.ai_base_url.clear();
            self.ai_api_key_source = "env".to_string();
            self.ai_api_key.clear();
            self.ai_api_key_env = "OPENAI_API_KEY".to_string();
        }
        self.ai_embedding_provider
            .clone_from(&ai_cfg.tasks.embedding.provider);
        if ene_ai::AiConfig::is_local_provider(&ai_cfg.tasks.embedding.provider) {
            self.ai_embedding_provider.clone_from(&ai_cfg.local_engine);
        }
        self.ai_embedding_model = ai_cfg.tasks.embedding.model.clone().unwrap_or_default();
        self.ai_embedding_dimensions =
            if ene_ai::LOCAL_ENGINE_CHOICES.contains(&self.ai_embedding_provider.as_str()) {
                "auto".to_string()
            } else {
                ai_cfg
                    .tasks
                    .embedding
                    .dimensions
                    .map_or_else(|| "1536".to_string(), |d| d.to_string())
            };

        self.tts_provider.clone_from(&ai_cfg.tts.provider);
        self.stt_provider.clone_from(&ai_cfg.stt.provider);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CharacterSettings;

    fn settings_with_inline_key() -> CharacterSettings {
        let tmp = std::env::temp_dir().join(format!("ene-input-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("test temp dir");
        let settings = CharacterSettings::discover(&tmp, "Alicia");
        settings.with_config_mut(|config| {
            let mut ai = config.get_section::<ene_ai::AiConfig>().unwrap_or_default();
            let provider = ai.tasks.chat.provider.clone();
            let def = ai
                .providers
                .entry(provider.clone())
                .or_insert_with(ene_ai::AiProviderDef::default);
            def.api_key.source = "inline".to_string();
            def.api_key.inline = "sk-super-secret".to_string();
            drop(config.set_section(&ai));
        });
        settings
    }

    #[test]
    fn stored_secrets_never_round_trip_into_ui_buffers() {
        let settings = settings_with_inline_key();
        let mut input = SettingsInputState::new();
        input.sync_from_settings(&settings, &crate::settings::UiState::default());
        assert!(
            input.ai_api_key.is_empty(),
            "the stored inline key must never be copied into a UI text buffer"
        );
        assert_eq!(input.ai_api_key_source, "inline");
    }
}
