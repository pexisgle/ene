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
    /// Last successfully fetched value, if any.
    pub data: Option<T>,
    /// Fetch failure message, if any.
    pub error: Option<String>,
}

impl<T> AsyncData<T> {
    /// Creates an idle fetch slot.
    #[must_use]
    pub fn new() -> Self {
        Self {
            receiver: None,
            data: None,
            error: None,
        }
    }

    /// Starts tracking an in-flight fetch. A request is only accepted when
    /// nothing is already in flight or cached.
    pub fn start(&mut self, receiver: tokio::sync::oneshot::Receiver<T>) {
        if self.receiver.is_none() && self.data.is_none() {
            self.receiver = Some(receiver);
            self.error = None;
        }
    }

    /// Drains a completed fetch (non-blocking).
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

    /// Whether a fetch is in flight.
    #[must_use]
    pub const fn loading(&self) -> bool {
        self.receiver.is_some()
    }

    /// Whether a fetch has ever been requested (used for first-render
    /// lazy loading).
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
    pub tts_model: String,
    pub tts_voice: String,
    pub tts_language: String,
    pub tts_model_path: String,
    pub tts_voices_path: String,
    pub stt_provider: String,
    pub stt_model: String,
    pub stt_language: String,
    /// Enumerated input device names for the microphone picker. Filled by
    /// the Voice page (and the Features audio section) when the window is
    /// shown; empty without the `voice` feature.
    pub mic_devices: Vec<String>,
    /// Cached `/models` catalog per provider key, fetched on demand by the
    /// AI page's refresh button.
    pub model_catalog: BTreeMap<String, Vec<String>>,
    /// Last `/models` fetch error message, if any.
    pub model_fetch_error: Option<String>,
    /// Asynchronously cached plugin settings snapshots for the plugin
    /// center page (and Overview / search), refreshed on window open and by
    /// the page's refresh button.
    pub plugin_snapshots: AsyncData<Vec<ene_plugin_host::PluginSettingsSnapshot>>,
    /// Asynchronously cached host-side artifact snapshot for the Engines
    /// page (installed sidecars/models plus catalog targets).
    pub artifact_snapshot: AsyncData<Vec<ene_plugin_host::ArtifactSnapshot>>,
    /// In-flight artifact installs/updates, keyed by artifact id.
    pub artifact_installs: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Receiver<Result<ene_plugin_host::InstalledArtifactView, String>>,
    >,
    /// In-flight artifact rollbacks, keyed by artifact id.
    pub artifact_rollbacks: std::collections::HashMap<
        String,
        tokio::sync::oneshot::Receiver<Result<ene_plugin_host::InstalledArtifactView, String>>,
    >,
    /// In-flight catalog refresh.
    pub catalog_refresh: Option<tokio::sync::oneshot::Receiver<Result<u64, String>>>,
    /// Two-step delete arms for model files on the Engines page
    /// (`plugin|model` → armed).
    pub model_delete_arm: std::collections::HashMap<String, bool>,
    /// Asynchronously cached schedule list for the Schedules page.
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
    /// Asynchronous plugin `ValidateConfig` results per plugin.
    pub plugin_validation: std::collections::BTreeMap<String, AsyncData<Vec<String>>>,
    /// Standing permission grants (Permission Center page).
    pub permissions: AsyncData<Vec<ene_runtime::PermissionScope>>,
    /// Message from the last permission mutation (revoke / reset).
    pub permission_action: AsyncData<String>,
    /// Registered connector summaries (Connectors page).
    pub connectors: AsyncData<Vec<ene_connector::ConnectorSummary>>,
    /// Status + grants of the selected connector.
    pub connector_detail: AsyncData<(
        Option<ene_connector::ConnectorStatus>,
        Vec<ene_connector::PermissionGrant>,
    )>,
    /// In-flight connector connectivity check result.
    pub connector_check: AsyncData<Result<ene_connector::HealthStatus, String>>,
    /// Session metadata rows (Sessions page).
    pub sessions: AsyncData<Vec<ene_runtime::PublicSessionMeta>>,
    /// Session message search results.
    pub session_search: AsyncData<Vec<(String, ene_runtime::PublicExportedMessage)>>,
    /// In-flight session mutation result message.
    pub session_message: AsyncData<String>,
    /// Provider catalog for the Voice page.
    pub provider_catalog: AsyncData<Option<ene_runtime::ProviderCatalog>>,
    /// `/models` fetch result for the AI page.
    pub model_list: AsyncData<Vec<String>>,
    /// API key test-connection result for the AI page.
    pub api_test: AsyncData<Result<(), String>>,
    /// Direct tool-call test results per plugin (e.g. Home Assistant
    /// connectivity).
    pub plugin_tool_test: std::collections::BTreeMap<String, AsyncData<String>>,
    /// In-flight memory-ledger mutation messages (polled each frame).
    pub ledger_pending: Vec<tokio::sync::oneshot::Receiver<String>>,
    /// MCP server liveness statuses (plugin center).
    pub mcp_statuses: AsyncData<Vec<ene_plugin_host::McpServerStatus>>,
}

impl SettingsInputState {
    pub fn new() -> Self {
        Self {
            ai_embedding_provider: "cloud".to_string(),
            ai_embedding_model: "text-embedding-3-small".to_string(),
            ai_embedding_dimensions: "1536".to_string(),
            ai_api_key_source: "env".to_string(),
            ai_api_key_env: "OPENAI_API_KEY".to_string(),
            ..Self::default()
        }
    }

    /// Mirror the on-disk `CharacterSettings` into the editable
    /// text buffers. Called when the settings window becomes
    /// visible.
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
        self.ai_embedding_provider =
            if ene_ai::AiConfig::is_local_provider(&ai_cfg.tasks.embedding.provider) {
                "local".to_string()
            } else {
                "cloud".to_string()
            };
        self.ai_embedding_model = ai_cfg.tasks.embedding.model.clone().unwrap_or_default();
        self.ai_embedding_dimensions = if self.ai_embedding_provider == "local" {
            "auto".to_string()
        } else {
            ai_cfg
                .tasks
                .embedding
                .dimensions
                .map_or_else(|| "1536".to_string(), |d| d.to_string())
        };

        self.tts_provider.clone_from(&ai_cfg.tts.provider);
        self.tts_model.clone_from(&ai_cfg.tts.model);
        self.tts_voice.clone_from(&ai_cfg.tts.voice);
        self.tts_language.clone_from(&ai_cfg.tts.language);
        self.tts_model_path = ai_cfg.tts.model_path.clone().unwrap_or_default();
        self.tts_voices_path = settings.kokoro_voices_path();

        self.stt_provider.clone_from(&ai_cfg.stt.provider);
        self.stt_model.clone_from(&ai_cfg.stt.model);
        self.stt_language.clone_from(&ai_cfg.stt.language);
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
