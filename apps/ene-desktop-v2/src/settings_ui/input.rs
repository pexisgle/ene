//! Per-render scratch buffer for editable text fields.
//!
//! `SettingsInputState` mirrors the legacy
//! `apps/ene-desktop/src/settings_ui/mod.rs::SettingsInputState`. It
//! is owned by the [`SettingsUi`](super::SettingsUi) and
//! `sync_from_settings` is called whenever the settings window
//! transitions from hidden → visible.
use crate::settings::CharacterSettings;

#[derive(Debug, Default)]
pub struct SettingsInputState {
    #[allow(dead_code)]
    // `SettingsUi` reads it; the standalone struct keeps it for future direct access.
    pub current_page: super::PageKind,
    pub look_at_strength: String,
    pub model_scale: String,
    pub character_pos_x: String,
    pub character_pos_y: String,
    pub character_pos_z: String,
    pub ai_user_name: String,
    pub ai_runtime_rules: String,
    pub ai_base_url: String,
    pub ai_api_key: String,
    pub ai_chat_input: String,
    pub ai_memory_enabled: bool,
    pub ai_embedding_provider: String,
    pub ai_embedding_model: String,
    pub ai_embedding_dimensions: String,
    pub ai_provider_name: String,
    pub ai_model: String,
    pub ai_api_key_env: String,
}

impl SettingsInputState {
    pub fn new() -> Self {
        Self {
            current_page: super::PageKind::Character,
            ai_embedding_provider: "cloud".to_string(),
            ai_embedding_model: "jina-embeddings-v5-text-small".to_string(),
            ai_embedding_dimensions: "auto".to_string(),
            ..Self::default()
        }
    }

    /// Mirror the on-disk `CharacterSettings` into the editable
    /// text buffers. Called when the settings window becomes
    /// visible.
    pub fn sync_from_settings(&mut self, settings: &CharacterSettings) {
        self.look_at_strength = format!("{:.2}", settings.character_state.look_at_strength);
        self.model_scale = format!("{:.2}", settings.character_state.model_scale);
        self.character_pos_x = format!("{:+.2}", settings.character_state.character_position.x);
        self.character_pos_y = format!("{:+.2}", settings.character_state.character_position.y);
        self.character_pos_z = format!("{:+.2}", settings.character_state.character_position.z);
        self.ai_user_name = settings.ai.ai.user_name.clone();
        self.ai_runtime_rules = settings.ai.ai.runtime_rules.clone();
        let provider = settings
            .ai
            .ai
            .get_section::<ene_core::ProviderConfig>()
            .unwrap_or_default();
        self.ai_base_url = provider.base_url.clone();
        self.ai_api_key = provider.api_key.inline.clone();
        self.ai_chat_input = settings.ui.ai_chat_input.clone();
        let mem = settings
            .ai
            .ai
            .get_section::<ene_core::MemoryConfig>()
            .unwrap_or_default();
        self.ai_memory_enabled = mem.enabled;
        self.ai_embedding_provider = provider.embedding.backend.clone();
        self.ai_embedding_model = if provider.embedding.backend == "local" {
            provider.embedding.local.model.clone()
        } else {
            provider.embedding.cloud.model.clone()
        };
        self.ai_embedding_dimensions = if provider.embedding.backend == "local" {
            "auto".to_string()
        } else {
            provider.embedding.cloud.dimensions.to_string()
        };
        self.ai_provider_name = provider.name.clone();
        self.ai_model = provider.model.clone();
        self.ai_api_key_env = provider.api_key.env.clone();
    }
}
