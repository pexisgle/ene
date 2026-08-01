//! Per-render scratch buffer for editable text fields.
//!
//! `SettingsInputState` is owned by the [`SettingsUi`](super::SettingsUi) and
//! `sync_from_settings` is called whenever the settings window
//! transitions from hidden → visible.
use crate::settings::CharacterSettings;

#[derive(Debug, Default)]
pub struct SettingsInputState {
    pub look_at_strength: String,
    pub model_scale: String,
    pub character_pos_x: String,
    pub character_pos_y: String,
    pub character_pos_z: String,
    pub ai_user_name: String,
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
    pub stt_model_path: String,
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
        self.ai_chat_model = ai_cfg.tasks.chat.model.clone().unwrap_or_default();
        if let Some(def) = ai_cfg.providers.get(&ai_cfg.tasks.chat.provider) {
            self.ai_base_url.clone_from(&def.base_url);
            self.ai_api_key_source.clone_from(&def.api_key.source);
            self.ai_api_key.clone_from(&def.api_key.inline);
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
        self.tts_voices_path = ai_cfg.tts.voices_path.clone().unwrap_or_default();

        self.stt_provider.clone_from(&ai_cfg.stt.provider);
        self.stt_model.clone_from(&ai_cfg.stt.model);
        self.stt_language.clone_from(&ai_cfg.stt.language);
        self.stt_model_path = ai_cfg.stt.model_path.clone().unwrap_or_default();
    }
}
