//! Per-render scratch buffer for editable text fields and async core fetches.
#![expect(
    dead_code,
    reason = "input helpers stay for apply/refresh flows that are not yet driven from the UI"
)]
use std::collections::BTreeMap;

use ene_api::{ApprovalView, MemoryView, PluginView, ScheduleView, SessionView};
use serde_json::Value;

use crate::settings::CharacterSettings;
use crate::settings_ui::cloud_models::CloudModelListUi;
use crate::settings_ui::provider_assets::ProviderAssetsUi;

/// Dynamic `ListConfigOptions` results: dotted field path → (label, value)
/// choices.
pub type PluginOptionsMap = BTreeMap<String, Vec<(String, String)>>;

/// Asynchronously fetched UI data with loading / error / retry state.
///
/// The owning page calls [`AsyncData::start`] with a receiver produced by
/// [`crate::core_session::CoreSession::spawn_fetch`] and polls [`AsyncData::poll`]
/// every frame; the render loop never blocks on HTTP.
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

    /// Used by live-updating views that must render the most recent snapshot
    /// while the next fetch is in flight.
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
    pub ai_api_key: String,
    pub ai_chat_key_set: bool,
    pub ai_classifier_key: String,
    pub ai_embedding_key: String,
    pub ai_proactive_key: String,
    pub ai_classifier_key_set: bool,
    pub ai_embedding_key_set: bool,
    pub ai_proactive_key_set: bool,
    pub ai_tts_key: String,
    pub ai_stt_key: String,
    pub ai_tts_key_set: bool,
    pub ai_stt_key_set: bool,
    pub ai_embedding_provider: String,
    pub ai_embedding_model: String,
    pub ai_embedding_dimensions: String,
    pub ai_validation_message: Option<String>,
    pub tts_provider: String,
    pub stt_provider: String,
    /// Selected curated chat GGUF id (`gemma-4-e2b`, …).
    pub local_catalog_id: String,
    /// Custom chat `.gguf` path from the file picker.
    pub local_custom_path: String,
    /// Selected curated embedding GGUF id (`jina-v5-small`, …).
    pub embed_catalog_id: String,
    /// Custom embedding `.gguf` path from the file picker.
    pub embed_custom_path: String,
    pub provider_assets: ProviderAssetsUi,
    pub cloud_models: CloudModelListUi,
    /// Enumerated input device names for the microphone picker.
    pub mic_devices: Vec<String>,
    pub health: AsyncData<Result<String, String>>,
    pub plugins: AsyncData<Vec<PluginView>>,
    pub memories: AsyncData<Vec<MemoryView>>,
    pub sessions: AsyncData<Vec<SessionView>>,
    pub session_export: AsyncData<Result<Value, String>>,
    pub schedules: AsyncData<Vec<ScheduleView>>,
    pub schedule_name: String,
    pub schedule_spec: String,
    pub approvals: AsyncData<Vec<ApprovalView>>,
    pub core_settings: AsyncData<Result<Value, String>>,
    pub mcp: AsyncData<Result<ene_api::McpDocument, String>>,
    pub mcp_json: String,
    pub mcp_status: Option<String>,
    /// GGUF combo / key flags were filled from the current core settings fetch.
    pub ai_pickers_seeded: bool,
}

impl SettingsInputState {
    pub fn new() -> Self {
        Self {
            ai_embedding_provider: String::new(),
            ai_embedding_model: String::new(),
            ai_embedding_dimensions: "1536".to_string(),
            local_catalog_id: "gemma-4-e2b".to_string(),
            embed_catalog_id: "jina-v5-small".to_string(),
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
        self.ai_api_key.clear();
        self.ai_pickers_seeded = false;
        self.core_settings = AsyncData::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CharacterSettings;

    #[test]
    fn stored_secrets_never_round_trip_into_ui_buffers() {
        let tmp = std::env::temp_dir().join(format!("ene-input-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("test temp dir");
        let settings = CharacterSettings::discover(&tmp, "Alicia");
        settings.with_config_mut(|config| {
            drop(config.set_path("ai.tasks.chat.api_key", "\"sk-super-secret\""));
        });
        let mut input = SettingsInputState::new();
        input.sync_from_settings(&settings, &crate::settings::UiState::default());
        assert!(
            input.ai_api_key.is_empty(),
            "the stored inline key must never be copied into a UI text buffer"
        );
    }
}
